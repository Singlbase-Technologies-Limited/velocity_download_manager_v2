#!/usr/bin/env node
// `pnpm package:windows` (#81, docs/PACKAGING.md): the Windows installer, a per-user NSIS setup
// that runs as the release identity. Run after `pnpm native:build`. It
//   1. builds the engine, the torrent helper and the native host for release, as the release identity;
//   2. stages them in src-tauri/binaries/, and the notices and build-info.json in src-tauri/staging/;
//   3. runs `tauri build` with src-tauri/tauri.release.conf.json;
//   4. writes the installer's SHA-256 beside it, and checks what the installer holds when 7-Zip is
//      there (`--require-listing` makes a missing 7-Zip an error, as in CI).
// Tools run directly, never through a shell. The rules are in package-lib.mjs.
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { copyFileSync, existsSync, mkdirSync, readdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { root, toolCommand } from './checks-runner.mjs';
import { hostTriple, installerProblems, SIDECARS, sidecarBuildArgs, sidecarSource, STAGED_FILES, tauriBuildArgs } from './package-lib.mjs';

if (process.platform !== 'win32') {
  console.error('package:windows builds the Windows installer, on Windows only.');
  process.exit(1);
}
const requireListing = process.argv.includes('--require-listing');

function run(command, args, options = {}) {
  console.log(`> ${[command, ...args].join(' ')}`);
  const r = spawnSync(command, args, { cwd: root, stdio: 'inherit', windowsHide: true, ...options });
  if (r.error) throw r.error;
  if (r.status !== 0) throw new Error(`${command} exited with ${r.status}`);
}
const [cargo] = toolCommand('cargo');

const rustc = spawnSync('rustc', ['-vV'], { cwd: root, encoding: 'utf8', windowsHide: true });
if (rustc.status !== 0) throw new Error(`rustc -vV failed:\n${rustc.stderr}`);
const triple = hostTriple(rustc.stdout);

run(cargo, sidecarBuildArgs());

mkdirSync(join(root, 'src-tauri', 'binaries'), { recursive: true });
for (const { program } of SIDECARS) {
  copyFileSync(join(root, 'target', 'release', `${program}.exe`), join(root, sidecarSource(program, triple)));
}
const staging = join(root, 'src-tauri', 'staging');
mkdirSync(staging, { recursive: true });
const stagedFrom = { 'THIRD_PARTY_NOTICES.md': join(root, 'THIRD_PARTY_NOTICES.md'), 'build-info.json': join(root, 'target', 'native', triple, 'build-info.json') };
for (const file of STAGED_FILES) {
  if (!existsSync(stagedFrom[file])) throw new Error(`${stagedFrom[file]} is missing${file === 'build-info.json' ? ': run `pnpm native:build` first' : ''}`);
  copyFileSync(stagedFrom[file], join(staging, file));
}

run(process.execPath, [join(root, 'node_modules', '@tauri-apps', 'cli', 'tauri.js'), ...tauriBuildArgs()]);

const bundleDir = join(root, 'target', 'release', 'bundle', 'nsis');
const installers = readdirSync(bundleDir).filter((f) => f.endsWith('-setup.exe'));
if (installers.length !== 1) throw new Error(`expected one installer in ${bundleDir}, found: ${installers.join(', ') || 'none'}`);
const installer = join(bundleDir, installers[0]);
const sha256 = createHash('sha256').update(readFileSync(installer)).digest('hex');
writeFileSync(`${installer}.sha256`, `${sha256}  ${installers[0]}\n`);
console.log(`Installer: ${installer}\nSHA-256:   ${sha256}`);

const listing = spawnSync('7z', ['l', '-slt', installer], { encoding: 'utf8', windowsHide: true, maxBuffer: 64 * 1024 * 1024 });
if (listing.error || listing.status !== 0) {
  const why = listing.error ? listing.error.message : listing.stderr;
  if (requireListing) throw new Error(`7-Zip couldn't list the installer: ${why}`);
  console.log(`Installer contents: NOT RUN (7-Zip isn't available: ${why.trim()})`);
} else {
  const problems = installerProblems(listing.stdout);
  if (problems.length) {
    console.error(`The installer is wrong:\n${problems.map((p) => `- ${p}`).join('\n')}`);
    process.exit(1);
  }
  console.log('Installer contents: PASS (every program and file present, no aria2)');
}
