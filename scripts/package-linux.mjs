#!/usr/bin/env node
// `pnpm package:linux` (#82, docs/PACKAGING.md): the Linux packages, a deb and an rpm that run as
// the release identity. No AppImage (PO decision on #82). Run after `pnpm native:build`, on Ubuntu
// 22.04 for packages that run there and on anything newer. It
//   1. builds and stages the sidecars and files, and runs `tauri build`, as package:windows does
//      (package-build.mjs);
//   2. writes each package's SHA-256 beside it;
//   3. checks what each package installs: every program and file, no aria2 and no path of V1's;
//   4. checks the programs in the deb: no system copy of a library V2 links statically, and no
//      glibc newer than Ubuntu 22.04's.
// The checks need dpkg-deb, rpm, readelf and objdump. Without one a check is NOT RUN;
// `--require-listing` makes that an error, as in CI. Tools run directly, never through a shell.
import { spawnSync } from 'node:child_process';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { buildPackages, finishPackage } from './package-build.mjs';
import { glibcProblem, linuxPackageFiles, linuxPackageProblems, neededLibraries, neededProblems, newestGlibc, packagePaths } from './package-lib.mjs';

if (process.platform !== 'linux') {
  console.error('package:linux builds the Linux packages, on Linux only.');
  process.exit(1);
}
const requireListing = process.argv.includes('--require-listing');

/** A tool's output, or null with the reason printed as NOT RUN (an error with --require-listing). */
function output(check, command, args) {
  const r = spawnSync(command, args, { encoding: 'utf8', maxBuffer: 64 * 1024 * 1024 });
  if (r.error || r.status !== 0) {
    const why = (r.error ? r.error.message : r.stderr).trim();
    if (requireListing) throw new Error(`${check}: ${command} failed: ${why}`);
    console.log(`${check}: NOT RUN (${command} failed: ${why})`);
    return null;
  }
  return r.stdout;
}

let failed = false;
function report(check, problems) {
  if (problems.length) {
    failed = true;
    console.error(`${check}: FAIL\n${problems.map((p) => `- ${p}`).join('\n')}`);
  } else {
    console.log(`${check}: PASS`);
  }
}

buildPackages('linux');
const deb = finishPackage('deb', '.deb');
const rpm = finishPackage('rpm', '.rpm');

for (const [check, command, args] of [
  ['deb contents', 'dpkg-deb', ['-c', deb]],
  ['rpm contents', 'rpm', ['-qlp', rpm]],
]) {
  const listing = output(check, command, args);
  if (listing !== null) report(check, linuxPackageProblems(packagePaths(listing)));
}

// The deb and the rpm carry the same programs, built once; the deb's are checked.
const extracted = mkdtempSync(join(tmpdir(), 'velocity-deb-'));
try {
  if (output('deb programs', 'dpkg-deb', ['-x', deb, extracted]) !== null) {
    const libraries = [];
    const glibc = [];
    let complete = true;
    for (const file of linuxPackageFiles().filter((f) => f.startsWith('usr/bin/'))) {
      const readelf = output('Libraries the programs load', 'readelf', ['-d', join(extracted, file)]);
      const objdump = output('glibc the programs need', 'objdump', ['-T', join(extracted, file)]);
      if (readelf === null || objdump === null) {
        complete = false;
        break;
      }
      libraries.push(...neededProblems(file, neededLibraries(readelf)));
      const problem = glibcProblem(file, newestGlibc(objdump));
      if (problem) glibc.push(problem);
    }
    if (complete) {
      report('Libraries the programs load', libraries);
      report('glibc the programs need', glibc);
    }
  }
} finally {
  rmSync(extracted, { recursive: true, force: true });
}

if (failed) process.exit(1);
