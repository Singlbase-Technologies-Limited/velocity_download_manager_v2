#!/usr/bin/env node
// `pnpm check:fast` — verification level L1: formatting, lint, type checks and unit tests.
// Every check runs even if an earlier one fails, so one run reports all problems; the command
// still exits non-zero if any check failed (it never turns a failure into a pass).
import { spawnSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { homedir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const exe = process.platform === 'win32' ? '.exe' : '';
const cargoOnPath = spawnSync('cargo', ['--version'], { windowsHide: true }).status === 0;
const cargo = cargoOnPath ? 'cargo' : join(homedir(), '.cargo', 'bin', `cargo${exe}`);
const nodeBin = (pkgPath) => [process.execPath, join(root, 'node_modules', pkgPath)];

const checks = [
  { name: 'Rust formatting (cargo fmt --check)', cmd: [cargo, 'fmt', '--all', '--', '--check'] },
  { name: 'Rust lint (cargo clippy -D warnings)', cmd: [cargo, 'clippy', '--workspace', '--all-targets', '--', '-D', 'warnings'] },
  { name: 'Rust unit tests (cargo test)', cmd: [cargo, 'test', '--workspace', '--no-fail-fast'] },
  { name: 'TypeScript types (tsc --noEmit)', cmd: [...nodeBin('typescript/bin/tsc'), '--noEmit'] },
  { name: 'ESLint (zero warnings)', cmd: [...nodeBin('eslint/bin/eslint.js'), '.', '--max-warnings', '0'] },
  { name: 'Vitest unit tests', cmd: [...nodeBin('vitest/vitest.mjs'), 'run'] },
];

if (!cargoOnPath && !existsSync(cargo)) {
  console.error('cargo not found. Run `pnpm check:prereqs` for installation instructions.');
  process.exit(1);
}
if (!existsSync(join(root, 'node_modules'))) {
  console.error('node_modules is missing. Run `pnpm install --frozen-lockfile` first.');
  process.exit(1);
}

const results = [];
for (const check of checks) {
  console.log(`\n=== ${check.name} ===`);
  const started = Date.now();
  const r = spawnSync(check.cmd[0], check.cmd.slice(1), { cwd: root, stdio: 'inherit', windowsHide: true });
  results.push({ name: check.name, ok: r.status === 0, seconds: (Date.now() - started) / 1000 });
}

console.log('\n=== check:fast summary ===');
for (const r of results) console.log(`${r.ok ? 'PASS' : 'FAIL'}  ${r.name}  (${r.seconds.toFixed(1)} s)`);
const failed = results.filter((r) => !r.ok).length;
console.log(failed ? `\n${failed} check(s) failed.` : '\nAll checks passed.');
process.exit(failed ? 1 : 0);
