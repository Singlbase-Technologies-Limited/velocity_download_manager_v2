// Runs check commands from checks-lib.mjs and prints a PASS/FAIL summary. Every check runs even
// if an earlier one fails, so one run reports all problems; the process still exits non-zero if
// any check failed (it never turns a failure into a pass). Tools run directly, never through a
// shell, so no argument is ever interpreted by one.
import { spawnSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { homedir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { workspaceFromMetadata } from './checks-lib.mjs';

export const root = join(dirname(fileURLToPath(import.meta.url)), '..');

const exe = process.platform === 'win32' ? '.exe' : '';
const cargoOnPath = spawnSync('cargo', ['--version'], { windowsHide: true }).status === 0;
const cargo = cargoOnPath ? 'cargo' : join(homedir(), '.cargo', 'bin', `cargo${exe}`);
const nodeBin = (pkgPath) => [process.execPath, join(root, 'node_modules', pkgPath)];

const tools = {
  cargo: () => [cargo],
  tsc: () => nodeBin('typescript/bin/tsc'),
  eslint: () => nodeBin('eslint/bin/eslint.js'),
  vitest: () => nodeBin('vitest/vitest.mjs'),
  vite: () => nodeBin('vite/bin/vite.js'),
};

/** Stops with a hint when cargo or node_modules is missing. */
export function requirePrerequisites() {
  if (!cargoOnPath && !existsSync(cargo)) {
    console.error('cargo not found. Run `pnpm check:prereqs` for installation instructions.');
    process.exit(1);
  }
  if (!existsSync(join(root, 'node_modules'))) {
    console.error('node_modules is missing. Run `pnpm install --frozen-lockfile` first.');
    process.exit(1);
  }
}

/** Reads the workspace crates and their dependencies from `cargo metadata`. */
export function loadWorkspace() {
  const r = spawnSync(cargo, ['metadata', '--format-version', '1', '--no-deps'], {
    cwd: root,
    encoding: 'utf8',
    maxBuffer: 64 * 1024 * 1024,
    windowsHide: true,
  });
  if (r.status !== 0) throw new Error(`cargo metadata failed:\n${r.stderr}`);
  return workspaceFromMetadata(JSON.parse(r.stdout));
}

/** Runs the commands in order and exits with 0 only if all passed. */
export function runAndExit(title, commands) {
  const results = [];
  for (const c of commands) {
    console.log(`\n=== ${c.name} ===`);
    const [bin, ...pre] = tools[c.tool]();
    const started = Date.now();
    const r = spawnSync(bin, [...pre, ...c.args], { cwd: root, stdio: 'inherit', windowsHide: true });
    results.push({ name: c.name, ok: r.status === 0, seconds: (Date.now() - started) / 1000 });
  }
  console.log(`\n=== ${title} summary ===`);
  for (const r of results) console.log(`${r.ok ? 'PASS' : 'FAIL'}  ${r.name}  (${r.seconds.toFixed(1)} s)`);
  const failed = results.filter((r) => !r.ok).length;
  console.log(failed ? `\n${failed} check(s) failed.` : '\nAll checks passed.');
  process.exit(failed ? 1 : 0);
}
