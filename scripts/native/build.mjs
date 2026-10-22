#!/usr/bin/env node
// Builds V2's native libraries from the sources in deps.lock, for the Rust host target (#35,
// docs/BUILDING_NATIVE.md), and writes target/native/<target>/build-info.json.
//
//   node scripts/native/build.mjs [--force]
//
// Windows (x86_64-pc-windows-msvc): zlib, nghttp2 and curl with Schannel, built with CMake and
// Visual Studio. Linux (x86_64-unknown-linux-gnu): zlib, nghttp2, a static OpenSSL and curl, built
// with each project's own configure script. Everything is static, and libcurl keeps only the
// protocols V2 uses (native-lib.mjs, ALLOWED_PROTOCOLS).
//
// A finished build is reused while its cache key (the lock file and these scripts) is unchanged
// and its artefacts still match their recorded SHA-256; --force rebuilds anyway. Every command
// runs with an argument list, never through a shell.
import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { availableParallelism, tmpdir } from 'node:os';
import { dirname, isAbsolute, join, relative, resolve } from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { checkBuildInfo, checkLinked, createBuildInfo, parseCurlVersion } from './native-lib.mjs';
import { NATIVE_DIR, REPO, fetchSources, hostTarget } from './fetch.mjs';

const HERE = dirname(fileURLToPath(import.meta.url));
const JOBS = String(availableParallelism());

/** curl protocols and features V2 never uses; disabling them keeps them unreachable by any URL or redirect. */
const DISABLED_CURL_FEATURES = ['dict', 'file', 'gopher', 'imap', 'ipfs', 'ldap', 'ldaps', 'mqtt', 'pop3', 'rtsp', 'smtp', 'telnet', 'tftp', 'websockets', 'netrc'];

/** Runs a program with an argument list (no shell) and fails loudly with its exit status. */
function run(cmd, args, opts = {}) {
  console.log(`$ ${[cmd, ...args].join(' ')}`);
  const r = spawnSync(cmd, args, { stdio: 'inherit', ...opts });
  if (r.error) throw new Error(`${cmd} could not start: ${r.error.message}`);
  if (r.status !== 0) throw new Error(`${cmd} ${args[0] ?? ''} failed with exit code ${r.status}`);
}

function output(cmd, args, opts = {}) {
  const r = spawnSync(cmd, args, { encoding: 'utf8', ...opts });
  if (r.error || r.status !== 0) throw new Error(`${cmd} ${args.join(' ')} failed: ${r.error?.message ?? r.stderr}`);
  return r.stdout;
}

const sha256 = (data) => createHash('sha256').update(data).digest('hex');
const firstLine = (s) => s.split(/\r?\n/)[0].trim();

/** Deletes a directory only when it lies inside target/native, the folder this build owns. */
function cleanOwnedDir(dir) {
  const rel = relative(NATIVE_DIR, resolve(dir));
  // `relative` returns an absolute path when `dir` is on another Windows drive.
  if (!rel || rel.startsWith('..') || isAbsolute(rel)) throw new Error(`refusing to delete ${dir}: it is outside ${NATIVE_DIR}`);
  rmSync(dir, { recursive: true, force: true });
  mkdirSync(dir, { recursive: true });
}

/** Unpacks an archive into `dir` and returns the single top-level folder it creates. */
function extract(archive, dir) {
  // A relative archive path works for both Windows' bsdtar and an MSYS tar found first on PATH.
  const tar = process.platform === 'win32' ? join(process.env.SystemRoot ?? 'C:\\Windows', 'System32', 'tar.exe') : 'tar';
  run(tar, ['-xf', relative(dir, archive)], { cwd: dir });
  const name = readdirSync(dir).find((d) => archive.endsWith(`${d}.tar.xz`) || archive.endsWith(`${d}.tar.gz`));
  if (!name) throw new Error(`${archive} did not unpack into the expected folder`);
  return join(dir, name);
}

// ---------- Windows: CMake with the Visual Studio generator ----------

/** CMake and the Visual Studio generator: cmake on PATH, else the copy Visual Studio installs. */
function windowsTools() {
  const vswhere = join(process.env['ProgramFiles(x86)'] ?? 'C:\\Program Files (x86)', 'Microsoft Visual Studio', 'Installer', 'vswhere.exe');
  if (!existsSync(vswhere)) throw new Error('Visual Studio was not found (no vswhere.exe). Install Visual Studio 2022 Build Tools with "Desktop development with C++"');
  const vs = firstLine(output(vswhere, ['-latest', '-products', '*', '-requires', 'Microsoft.VisualStudio.Component.VC.Tools.x86.x64', '-property', 'installationPath']));
  const major = firstLine(output(vswhere, ['-latest', '-products', '*', '-requires', 'Microsoft.VisualStudio.Component.VC.Tools.x86.x64', '-property', 'catalog_productLineVersion']));
  if (!vs) throw new Error('Visual Studio has no C++ toolset. Add "Desktop development with C++" in the Visual Studio Installer');
  const generators = { 2022: 'Visual Studio 17 2022', 2026: 'Visual Studio 18 2026' };
  const generator = generators[major];
  if (!generator) throw new Error(`Visual Studio ${major} has no known CMake generator here (known: ${Object.keys(generators).join(', ')})`);
  let cmake = 'cmake';
  if (spawnSync(cmake, ['--version'], { stdio: 'ignore' }).status !== 0) {
    cmake = join(vs, 'Common7', 'IDE', 'CommonExtensions', 'Microsoft', 'CMake', 'CMake', 'bin', 'cmake.exe');
    if (!existsSync(cmake)) throw new Error('CMake was not found on PATH or in Visual Studio. Install the "C++ CMake tools for Windows" component');
  }
  return { cmake, generator };
}

function buildWindows(sources, work, prefix) {
  const { cmake, generator } = windowsTools();
  const P = prefix.replaceAll('\\', '/');
  const common = ['-G', generator, '-A', 'x64', `-DCMAKE_INSTALL_PREFIX=${P}`, `-DCMAKE_PREFIX_PATH=${P}`];
  const cmakeBuild = (name, src, opts) => {
    const dir = join(work, `${name}-build`);
    run(cmake, ['-S', src, '-B', dir, ...common, ...opts]);
    run(cmake, ['--build', dir, '--config', 'Release', '--parallel', JOBS]);
    run(cmake, ['--install', dir, '--config', 'Release']);
    return dir;
  };
  const options = {
    zlib: ['-DZLIB_BUILD_SHARED=OFF', '-DZLIB_BUILD_STATIC=ON', '-DZLIB_BUILD_TESTING=OFF'],
    nghttp2: ['-DENABLE_LIB_ONLY=ON', '-DBUILD_SHARED_LIBS=OFF', '-DBUILD_STATIC_LIBS=ON', '-DENABLE_DOC=OFF', '-DBUILD_TESTING=OFF'],
  };
  cmakeBuild('zlib', extract(sources.zlib, work), options.zlib);
  cmakeBuild('nghttp2', extract(sources.nghttp2, work), options.nghttp2);
  const zlibLib = ['zs.lib', 'zlibstatic.lib', 'zlib.lib'].map((f) => join(prefix, 'lib', f)).find(existsSync);
  if (!zlibLib) throw new Error(`the zlib build installed no static library in ${join(prefix, 'lib')}`);
  options.curl = [
    '-DBUILD_SHARED_LIBS=OFF',
    '-DBUILD_STATIC_LIBS=ON',
    '-DBUILD_CURL_EXE=ON',
    '-DBUILD_STATIC_CURL=ON',
    '-DBUILD_TESTING=OFF',
    '-DBUILD_EXAMPLES=OFF',
    '-DBUILD_LIBCURL_DOCS=OFF',
    '-DBUILD_MISC_DOCS=OFF',
    '-DENABLE_CURL_MANUAL=OFF',
    '-DCURL_USE_PKGCONFIG=OFF',
    '-DCURL_USE_SCHANNEL=ON',
    '-DCURL_USE_OPENSSL=OFF',
    '-DUSE_NGHTTP2=ON',
    '-DCURL_ZLIB=ON',
    `-DZLIB_LIBRARY=${zlibLib.replaceAll('\\', '/')}`,
    `-DZLIB_INCLUDE_DIR=${P}/include`,
    '-DCURL_BROTLI=OFF',
    '-DCURL_ZSTD=OFF',
    '-DUSE_LIBIDN2=OFF',
    '-DUSE_WIN32_IDN=OFF',
    '-DCURL_USE_LIBPSL=OFF',
    '-DCURL_USE_LIBSSH2=OFF',
    '-DCURL_USE_LIBSSH=OFF',
    '-DCURL_USE_GSSAPI=OFF',
    ...DISABLED_CURL_FEATURES.map((f) => `-DCURL_DISABLE_${f.toUpperCase()}=ON`),
  ];
  const curlBuild = cmakeBuild('curl', extract(sources.curl, work), options.curl);
  return { options, curlTool: join(prefix, 'bin', 'curl.exe'), compilerProbe: curlBuild, cmake };
}

// ---------- Linux: each project's configure script ----------

function buildLinux(sources, work, prefix) {
  const env = { ...process.env, CFLAGS: '-O2 -fPIC', PKG_CONFIG_PATH: join(prefix, 'lib', 'pkgconfig'), PKG_CONFIG_LIBDIR: join(prefix, 'lib', 'pkgconfig') };
  const make = (cwd, target = []) => run('make', ['-j', JOBS, ...target], { cwd, env });
  const options = {
    zlib: [`--prefix=${prefix}`, '--static'],
    nghttp2: [`--prefix=${prefix}`, '--enable-lib-only', '--disable-shared', '--enable-static', '--with-pic'],
    // `--openssldir` is where OpenSSL looks for its configuration and default trust store at run time.
    openssl: ['linux-x86_64', `--prefix=${prefix}`, '--libdir=lib', '--openssldir=/etc/ssl', 'no-shared', 'no-tests', 'no-docs'],
    curl: [
      `--prefix=${prefix}`,
      '--disable-shared',
      '--enable-static',
      '--with-pic',
      `--with-openssl=${prefix}`,
      `--with-nghttp2=${prefix}`,
      `--with-zlib=${prefix}`,
      '--disable-openssl-auto-load-config',
      '--enable-threaded-resolver',
      '--with-ca-bundle=/etc/ssl/certs/ca-certificates.crt',
      '--with-ca-path=/etc/ssl/certs',
      '--without-libpsl',
      '--without-libidn2',
      '--without-brotli',
      '--without-zstd',
      '--without-libssh2',
      '--without-libssh',
      '--without-librtmp',
      '--without-gssapi',
      '--disable-docs',
      '--disable-manual',
      ...DISABLED_CURL_FEATURES.map((f) => `--disable-${f}`),
    ],
  };
  const zlib = extract(sources.zlib, work);
  run('./configure', options.zlib, { cwd: zlib, env });
  make(zlib, ['install']);
  const nghttp2 = extract(sources.nghttp2, work);
  run('./configure', options.nghttp2, { cwd: nghttp2, env });
  make(nghttp2, ['install']);
  const openssl = extract(sources.openssl, work);
  run('./Configure', options.openssl, { cwd: openssl, env });
  make(openssl);
  run('make', ['install_sw'], { cwd: openssl, env });
  const curl = extract(sources.curl, work);
  run('./configure', options.curl, { cwd: curl, env });
  make(curl, ['install']);
  return { options, curlTool: join(prefix, 'bin', 'curl') };
}

// ---------- Identity record ----------

function toolchains(target, extra) {
  const t = {
    rustc: firstLine(output('rustc', ['-V'])),
    node: process.version,
  };
  if (target.includes('windows')) {
    t.cmake = firstLine(output(extra.cmake, ['--version']));
    // CMake records the MSVC compiler it found in the build tree.
    const files = join(extra.compilerProbe, 'CMakeFiles');
    const dir = readdirSync(files).find((d) => /^\d+\.\d+/.test(d));
    const m = /CMAKE_C_COMPILER_VERSION "([^"]+)"/.exec(readFileSync(join(files, dir, 'CMakeCCompiler.cmake'), 'utf8'));
    t.cc = `MSVC ${m ? m[1] : 'unknown'}`;
  } else {
    t.cc = firstLine(output('cc', ['--version']));
    t.make = firstLine(output('make', ['--version']));
  }
  return t;
}

function v2Commit() {
  const commit = firstLine(output('git', ['rev-parse', 'HEAD'], { cwd: REPO }));
  const dirty = output('git', ['status', '--porcelain'], { cwd: REPO }).trim().length > 0;
  return { commit, dirty };
}

/** Changes to the pins or to how they're built invalidate a finished build. */
function cacheKey(target) {
  const h = createHash('sha256').update(target);
  for (const f of ['deps.lock', 'build.mjs', 'fetch.mjs', 'native-lib.mjs']) h.update(readFileSync(join(HERE, f)));
  return h.digest('hex');
}

function artefacts(prefix) {
  const files = [];
  for (const sub of ['lib', 'bin']) {
    const dir = join(prefix, sub);
    if (!existsSync(dir)) continue;
    for (const f of readdirSync(dir).sort()) {
      if (/\.(lib|a|exe)$/.test(f) || (sub === 'bin' && f === 'curl')) {
        files.push({ path: `${sub}/${f}`, sha256: sha256(readFileSync(join(dir, f))) });
      }
    }
  }
  return files;
}

/** A finished build can be reused when its key matches and its artefacts are unchanged. */
function reusable(outDir, key, lock) {
  const infoPath = join(outDir, 'build-info.json');
  if (!existsSync(infoPath)) return false;
  try {
    const info = JSON.parse(readFileSync(infoPath, 'utf8'));
    if (info.cacheKey !== key || checkBuildInfo(info, lock).length) return false;
    return info.artifacts.every((a) => existsSync(join(outDir, 'prefix', ...a.path.split('/'))) && sha256(readFileSync(join(outDir, 'prefix', ...a.path.split('/')))) === a.sha256);
  } catch {
    return false;
  }
}

async function main() {
  const force = process.argv.includes('--force');
  const target = hostTarget();
  const outDir = join(NATIVE_DIR, target);
  const prefix = join(outDir, 'prefix');
  const key = cacheKey(target);
  const { lock, files, signatures } = await fetchSources(target);
  if (!force && reusable(outDir, key, lock)) {
    console.log(`native build for ${target} is up to date: ${join(outDir, 'build-info.json')}`);
    return;
  }
  // On Windows, compiling under a deep checkout breaks MSVC's 260-character path limit (C1083
  // "Cannot open compiler generated file", seen in a clean clone 140 characters deep), so the
  // sources are built in a short temporary folder this run creates and always deletes.
  const windows = target === 'x86_64-pc-windows-msvc';
  const work = windows ? mkdtempSync(join(tmpdir(), 'v2n-')) : join(outDir, 'build');
  if (!windows) cleanOwnedDir(work);
  cleanOwnedDir(prefix);
  rmSync(join(outDir, 'build-info.json'), { force: true });
  try {
    buildAndRecord({ target, lock, files, signatures, work, prefix, outDir, key });
  } finally {
    rmSync(work, { recursive: true, force: true });
  }
}

function buildAndRecord({ target, lock, files, signatures, work, prefix, outDir, key }) {
  let built;
  if (target === 'x86_64-pc-windows-msvc') built = buildWindows(files, work, prefix);
  else if (target === 'x86_64-unknown-linux-gnu') built = buildLinux(files, work, prefix);
  else throw new Error(`no native build is established for ${target}`);

  const linked = parseCurlVersion(output(built.curlTool, ['-V']));
  const problems = checkLinked(target, lock, linked);
  if (problems.length) throw new Error(`the built libcurl doesn't match what V2 requires:\n- ${problems.join('\n- ')}`);

  const info = createBuildInfo({
    target,
    lock,
    signatures,
    options: built.options,
    linked,
    artifacts: artefacts(prefix),
    toolchains: toolchains(target, built),
    v2: v2Commit(),
    cacheKey: key,
  });
  const infoProblems = checkBuildInfo(info, lock);
  if (infoProblems.length) throw new Error(`build-info.json is incomplete:\n- ${infoProblems.join('\n- ')}`);
  writeFileSync(join(outDir, 'build-info.json'), `${JSON.stringify(info, null, 2)}\n`);
  console.log(`native build for ${target} finished: ${join(outDir, 'build-info.json')}`);
  console.log(`libcurl ${linked.libcurl}, ${linked.tls}, ${Object.entries(linked.libraries).map(([n, v]) => `${n} ${v}`).join(', ')}`);
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  main().catch((e) => {
    console.error(`native build failed: ${e instanceof Error ? e.message : e}`);
    process.exit(1);
  });
}

