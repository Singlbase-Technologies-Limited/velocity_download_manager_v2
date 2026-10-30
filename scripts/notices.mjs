#!/usr/bin/env node
// `pnpm notices` writes THIRD_PARTY_NOTICES.md from the dependency graphs (#76); `pnpm check:notices`
// fails when a component's licence isn't allowed (docs/DEPENDENCIES.md) or when the file is out of
// date. The rules are in notices-lib.mjs; this file only gathers the inputs:
// - Rust: `cargo tree` for the programs V2 ships, on every target, and `cargo metadata`;
// - npm: `pnpm licenses list --prod`;
// - native: scripts/native/deps.lock and the vcpkg overlay ports, with their licence texts in
//   scripts/native/licenses/ (copied from each verified source).
// Licence texts come from each package's own files; scripts/notices/licenses/ holds the SPDX
// standard texts for packages that ship none.
import { spawnSync } from 'node:child_process';
import { existsSync, readdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { root, toolCommand } from './checks-runner.mjs';
import {
  chooseLicence,
  crateComponents,
  licenceProblems,
  nativeComponents,
  normaliseText,
  npmComponents,
  parseDepsLock,
  pickLicenceFiles,
  renderNotices,
} from './notices-lib.mjs';

/** The packages whose programs V2 ships: the app, the engine, the torrent helper, the native host. */
const SHIPPED = ['velocity-desktop-v2', 'velocity-transfer', 'velocity-torrent', 'velocity-native-host'];
const OUTPUT = join(root, 'THIRD_PARTY_NOTICES.md');
const NATIVE = join(root, 'scripts', 'native');
const STANDARD = join(root, 'scripts', 'notices', 'licenses');

function run(name, args) {
  const [command, ...pre] = toolCommand(name);
  const r = spawnSync(command, [...pre, ...args], { cwd: root, encoding: 'utf8', maxBuffer: 256 * 1024 * 1024, windowsHide: true });
  if (r.error) throw r.error;
  if (r.status !== 0) throw new Error(`${name} ${args.join(' ')} failed:\n${r.stderr}`);
  return r.stdout;
}

const read = (path) => normaliseText(readFileSync(path, 'utf8'));

/** The licence texts a package ships, for the licences it is used under. */
function packageTexts(component) {
  const choice = chooseLicence(component.licence, component);
  if (!choice.ok || !existsSync(component.dir)) return [];
  return pickLicenceFiles(readdirSync(component.dir), choice.ids).map((f) => ({
    licence: choice.ids.join(' AND '),
    text: read(join(component.dir, f)),
  }));
}

const check = process.argv.includes('--check');

const metadata = JSON.parse(run('cargo', ['metadata', '--format-version', '1', '--locked']));
const tree = run('cargo', ['tree', '--locked', '-e', 'normal', '--target', 'all', '--prefix', 'none', '-f', '{p}', ...SHIPPED.flatMap((p) => ['-p', p])]);
const crates = crateComponents(metadata, tree).map((c) => ({ ...c, texts: packageTexts(c) }));
const npm = npmComponents(JSON.parse(run('pnpm', ['licenses', 'list', '--prod', '--json']))).map((c) => ({ ...c, texts: packageTexts(c) }));

const ports = readdirSync(join(NATIVE, 'vcpkg', 'ports')).map((name) => {
  const port = JSON.parse(readFileSync(join(NATIVE, 'vcpkg', 'ports', name, 'vcpkg.json'), 'utf8'));
  return { name: port.name, version: port.version };
});
const native = nativeComponents(parseDepsLock(readFileSync(join(NATIVE, 'deps.lock'), 'utf8')), ports).map((c) => {
  const file = join(NATIVE, 'licenses', `${c.name}.txt`);
  if (!existsSync(file)) throw new Error(`${c.name} has no licence text in scripts/native/licenses/`);
  return { ...c, authors: [], texts: [{ licence: c.licence, text: read(file) }] };
});

const standardTexts = Object.fromEntries(readdirSync(STANDARD).map((f) => [f.replace(/\.txt$/, ''), read(join(STANDARD, f))]));

const problems = licenceProblems([...native, ...crates, ...npm]);
const notices = `${renderNotices({ native, crates, npm, standardTexts })}\n`;
const summary = `${native.length} native libraries, ${crates.length} crates, ${npm.length} npm packages`;

if (problems.length) {
  console.error(`Licences that may not ship (docs/DEPENDENCIES.md, "Allowed licences"):\n${problems.map((p) => `- ${p}`).join('\n')}`);
}
if (check) {
  const current = existsSync(OUTPUT) ? readFileSync(OUTPUT, 'utf8').replace(/\r\n/g, '\n') : '';
  const stale = current !== notices;
  if (stale) console.error('THIRD_PARTY_NOTICES.md is out of date: run `pnpm notices` and commit the result.');
  if (problems.length || stale) process.exit(1);
  console.log(`THIRD_PARTY_NOTICES.md is up to date: ${summary}.`);
} else {
  writeFileSync(OUTPUT, notices);
  console.log(`THIRD_PARTY_NOTICES.md: ${summary}.`);
  if (problems.length) process.exit(1);
}
