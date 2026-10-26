#!/usr/bin/env node
// Builds libtorrent and its libraries with vcpkg for the Rust host target (#62,
// docs/BUILDING_NATIVE.md, "libtorrent") and writes target/native/<target>/libtorrent-info.json,
// the record crates/velocity-lt/build.rs compiles and links from.
//
//   node scripts/native/libtorrent.mjs [--force]      (also run by build.mjs)
//
// The manifest in scripts/native/vcpkg pins the vcpkg registry by its builtin-baseline commit, and
// its overlay ports pin libtorrent 2.0.15 and OpenSSL 3.5.9 by the SHA-512 of their sources; vcpkg
// checks those hashes on every download. A finished build is reused while its cache key (the
// manifest, the ports and these scripts) is unchanged and its libraries still match their
// recorded SHA-256; --force rebuilds anyway. Every command runs with an argument list, never
// through a shell.
import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, readFileSync, readdirSync, statSync, writeFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { dirname, join, relative } from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { LIBTORRENT_INFO_SCHEMA, TRIPLETS, checkLibtorrentInfo, librariesForLibtorrent, parsePackageVersion, parseTargets } from './libtorrent-lib.mjs';
import { NATIVE_DIR, hostTarget } from './fetch.mjs';

const HERE = dirname(fileURLToPath(import.meta.url));
const MANIFEST_DIR = join(HERE, 'vcpkg');
const INSTALL_ROOT = join(NATIVE_DIR, 'vcpkg-installed');

const sha256 = (data) => createHash('sha256').update(data).digest('hex');

/** Every file under `dir`, sorted, as paths relative to it. */
function filesUnder(dir) {
  const out = [];
  const walk = (d) => {
    for (const name of readdirSync(d).sort()) {
      const p = join(d, name);
      if (statSync(p).isDirectory()) walk(p);
      else out.push(relative(dir, p).replaceAll('\\', '/'));
    }
  };
  walk(dir);
  return out;
}

/** The vcpkg executable: VCPKG_ROOT when set, else the usual clone location for the platform. */
export function findVcpkg() {
  const exe = process.platform === 'win32' ? 'vcpkg.exe' : 'vcpkg';
  const roots = [process.env.VCPKG_ROOT, process.platform === 'win32' ? 'C:\\vcpkg' : join(homedir(), 'vcpkg')].filter(Boolean);
  for (const root of roots) {
    if (existsSync(join(root, exe))) return { root, exe: join(root, exe) };
  }
  throw new Error(
    `vcpkg was not found (looked in ${roots.join(', ')}). Clone https://github.com/microsoft/vcpkg, run its bootstrap script, and set VCPKG_ROOT (docs/BUILDING_NATIVE.md, "libtorrent")`,
  );
}

/** Changes to the manifest, the ports or how they're built invalidate a finished build. */
function cacheKey(target) {
  const h = createHash('sha256').update(target);
  for (const f of filesUnder(MANIFEST_DIR)) h.update(f).update(readFileSync(join(MANIFEST_DIR, f)));
  for (const f of ['libtorrent.mjs', 'libtorrent-lib.mjs']) h.update(readFileSync(join(HERE, f)));
  return h.digest('hex');
}

function artefacts(prefix, staticLibs) {
  const lib = join(prefix, 'lib');
  const file = (name) => [`${name}.lib`, `lib${name}.a`].find((f) => existsSync(join(lib, f)));
  return staticLibs.map((name) => {
    const f = file(name);
    if (!f) throw new Error(`the vcpkg build installed no static library ${name} in ${lib}`);
    return { path: `lib/${f}`, sha256: sha256(readFileSync(join(lib, f))) };
  });
}

function reusable(infoPath, key) {
  if (!existsSync(infoPath)) return false;
  try {
    const info = JSON.parse(readFileSync(infoPath, 'utf8'));
    if (info.cacheKey !== key || checkLibtorrentInfo(info).length) return false;
    return info.artifacts.every((a) => {
      const p = join(info.prefix, ...a.path.split('/'));
      return existsSync(p) && sha256(readFileSync(p)) === a.sha256;
    });
  } catch {
    return false;
  }
}

/** Builds (or reuses) libtorrent for `target` and returns the path of its record. */
export function buildLibtorrent({ target = hostTarget(), force = false, log = console.log } = {}) {
  const triplet = TRIPLETS[target];
  if (!triplet) throw new Error(`no libtorrent build is established for ${target}`);
  const infoPath = join(NATIVE_DIR, target, 'libtorrent-info.json');
  const key = cacheKey(target);
  if (!force && reusable(infoPath, key)) {
    log(`libtorrent for ${target} is up to date: ${infoPath}`);
    return infoPath;
  }

  const vcpkg = findVcpkg();
  const args = ['install', '--triplet', triplet, `--x-manifest-root=${MANIFEST_DIR}`, `--x-install-root=${INSTALL_ROOT}`, '--clean-after-build'];
  log(`$ ${[vcpkg.exe, ...args].join(' ')}`);
  const r = spawnSync(vcpkg.exe, args, { stdio: 'inherit', env: { ...process.env, VCPKG_DISABLE_METRICS: '1' } });
  if (r.error) throw new Error(`vcpkg could not start: ${r.error.message}`);
  if (r.status !== 0) throw new Error(`vcpkg install failed with exit code ${r.status}`);

  const prefix = join(INSTALL_ROOT, triplet);
  const share = join(prefix, 'share', 'LibtorrentRasterbar');
  const { defines, links } = parseTargets(readFileSync(join(share, 'LibtorrentRasterbarTargets.cmake'), 'utf8'));
  const { staticLibs, systemLibs } = librariesForLibtorrent(target, links);
  const manifest = JSON.parse(readFileSync(join(MANIFEST_DIR, 'vcpkg.json'), 'utf8'));
  const info = {
    schema: LIBTORRENT_INFO_SCHEMA,
    target,
    libtorrent: parsePackageVersion(readFileSync(join(share, 'LibtorrentRasterbarConfigVersion.cmake'), 'utf8')),
    triplet,
    vcpkgBaseline: manifest['builtin-baseline'],
    prefix,
    defines,
    staticLibs,
    systemLibs,
    artifacts: artefacts(prefix, staticLibs),
    cacheKey: key,
  };
  const problems = checkLibtorrentInfo(info);
  if (problems.length) throw new Error(`the libtorrent build doesn't match what V2 requires:\n- ${problems.join('\n- ')}`);
  mkdirSync(dirname(infoPath), { recursive: true });
  writeFileSync(infoPath, `${JSON.stringify(info, null, 2)}\n`);
  log(`libtorrent ${info.libtorrent} for ${target} finished: ${infoPath}`);
  return infoPath;
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  try {
    buildLibtorrent({ force: process.argv.includes('--force') });
  } catch (e) {
    console.error(`libtorrent build failed: ${e instanceof Error ? e.message : e}`);
    process.exit(1);
  }
}
