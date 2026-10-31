#!/usr/bin/env node
// `pnpm package:windows` (#81, docs/PACKAGING.md): the Windows installer, a per-user NSIS setup
// that runs as the release identity. Run after `pnpm native:build`. It
//   1. builds the engine, the torrent helper and the native host for release, as the release identity;
//   2. stages them in src-tauri/binaries/, and the notices and build-info.json in src-tauri/staging/;
//   3. runs `tauri build` with src-tauri/tauri.release.conf.json;
//   4. writes the installer's SHA-256 beside it, and checks what the installer holds when 7-Zip is
//      there (`--require-listing` makes a missing 7-Zip an error, as in CI).
// Steps 1 to 3 are package-build.mjs's, shared with `pnpm package:linux`. Tools run directly, never
// through a shell. The rules are in package-lib.mjs.
import { spawnSync } from 'node:child_process';
import { buildPackages, finishPackage } from './package-build.mjs';
import { installerProblems } from './package-lib.mjs';

if (process.platform !== 'win32') {
  console.error('package:windows builds the Windows installer, on Windows only.');
  process.exit(1);
}
const requireListing = process.argv.includes('--require-listing');

buildPackages('windows');
const installer = finishPackage('nsis', '-setup.exe');

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
