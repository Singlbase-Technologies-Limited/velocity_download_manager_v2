#!/usr/bin/env node
// `pnpm version:set <MAJOR.MINOR.PATCH>` (#84, docs/RELEASE_PROCESS.md): sets V2's version in
// package.json and Cargo.toml, then updates Cargo.lock's own crates (`cargo update --workspace`),
// nothing else in it. Tauri reads package.json's (tauri.conf.json), and a test fails while the
// files disagree (version-lib.test.mjs). Tools run directly, never through a shell.
import { readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { root, toolCommand } from './checks-runner.mjs';
import { run } from './package-build.mjs';
import { setVersion, versionProblems } from './version-lib.mjs';

const [command, version] = process.argv.slice(2);
if (command !== 'set' || !version) {
  console.error('usage: pnpm version:set <MAJOR.MINOR.PATCH>');
  process.exit(2);
}

const path = (file) => join(root, file);
const next = setVersion({ packageJson: readFileSync(path('package.json'), 'utf8'), cargoToml: readFileSync(path('Cargo.toml'), 'utf8') }, version);
writeFileSync(path('package.json'), next.packageJson);
writeFileSync(path('Cargo.toml'), next.cargoToml);

const [cargo] = toolCommand('cargo');
run(cargo, ['update', '--workspace']);

const problems = versionProblems({ ...next, tauriConf: readFileSync(path('src-tauri/tauri.conf.json'), 'utf8') });
if (problems.length) {
  console.error(problems.map((p) => `- ${p}`).join('\n'));
  process.exit(1);
}
console.log(`V2 is now ${version}: package.json, Cargo.toml and Cargo.lock. Commit them with the changelog (docs/RELEASE_PROCESS.md).`);
