// Runs check commands from checks-lib.mjs and prints a PASS/FAIL summary. Every check runs even
// if an earlier one fails, so one run reports all problems; the process still exits non-zero if
// any check failed (it never turns a failure into a pass). Tools run directly, never through a
// shell, so no argument is ever interpreted by one.
import { spawnSync } from 'node:child_process';
import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { workspaceFromMetadata } from './checks-lib.mjs';
import { extractErrors } from './diagnostics-lib.mjs';

export const root = join(dirname(fileURLToPath(import.meta.url)), '..');

const exe = process.platform === 'win32' ? '.exe' : '';
const cargoOnPath = spawnSync('cargo', ['--version'], { windowsHide: true }).status === 0;
const cargo = cargoOnPath ? 'cargo' : join(homedir(), '.cargo', 'bin', `cargo${exe}`);
const nodeBin = (pkgPath) => [process.execPath, join(root, 'node_modules', pkgPath)];

/** pnpm itself, as the `pnpm <script>` that started this check: `npm_execpath` names it. */
function pnpmBin() {
  const execPath = process.env.npm_execpath;
  if (!execPath) throw new Error('pnpm not found: run this check as `pnpm <script>`.');
  return execPath.endsWith('.js') || execPath.endsWith('.cjs') ? [process.execPath, execPath] : [execPath];
}

const tools = {
  cargo: () => [cargo],
  node: () => [process.execPath],
  tsc: () => nodeBin('typescript/bin/tsc'),
  eslint: () => nodeBin('eslint/bin/eslint.js'),
  vitest: () => nodeBin('vitest/vitest.mjs'),
  vite: () => nodeBin('vite/bin/vite.js'),
  pnpm: pnpmBin,
};

/** The command line that starts `name` (a key of `tools`), without a shell. */
export function toolCommand(name) {
  return tools[name]();
}

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

function git(...args) {
  const r = spawnSync('git', args, { cwd: root, encoding: 'utf8', windowsHide: true });
  if (r.status !== 0) throw new Error(`git ${args.join(' ')} failed:\n${r.stderr}`);
  return r.stdout;
}
const nulList = (out) => out.split('\0').filter(Boolean);

/**
 * Every path changed since the merge base of HEAD and `base`: committed, staged, unstaged and
 * untracked. Renames count as a deletion plus an addition, so both paths are judged.
 * @returns {{mergeBase: string, paths: string[]}}
 */
export function changedPathsSince(base) {
  const mergeBase = git('merge-base', 'HEAD', base).trim();
  const paths = new Set([
    ...nulList(git('diff', '--name-only', '--no-renames', '-z', mergeBase)),
    ...nulList(git('ls-files', '--others', '--exclude-standard', '-z')),
  ]);
  return { mergeBase, paths: [...paths] };
}

/**
 * Adds one check's result to the JSON report at $CI_REPORT_PATH, when set (CI, #91). The report
 * holds every check of the job with its status and the errors extracted from its output.
 */
export function recordCheck(check) {
  const path = process.env.CI_REPORT_PATH;
  if (!path) return;
  const report = existsSync(path) ? JSON.parse(readFileSync(path, 'utf8')) : { checks: [] };
  report.checks.push(check);
  writeFileSync(path, JSON.stringify(report, null, 2));
}

/** Runs the commands in order and exits with 0 only if all passed. */
export function runAndExit(title, commands) {
  const results = [];
  const capture = Boolean(process.env.CI_REPORT_PATH);
  for (const c of commands) {
    console.log(`\n=== ${c.name} ===`);
    const [bin, ...pre] = tools[c.tool]();
    const started = Date.now();
    // In CI the output is captured (and then printed) so the report can quote the errors.
    const r = spawnSync(bin, [...pre, ...c.args], {
      cwd: root,
      stdio: capture ? ['inherit', 'pipe', 'pipe'] : 'inherit',
      encoding: 'utf8',
      maxBuffer: 256 * 1024 * 1024,
      windowsHide: true,
    });
    if (capture) process.stdout.write(`${r.stdout ?? ''}${r.stderr ?? ''}`);
    const ok = r.status === 0;
    const seconds = (Date.now() - started) / 1000;
    results.push({ name: c.name, ok, seconds });
    recordCheck({
      id: c.id,
      name: c.name,
      status: ok ? 'pass' : 'fail',
      seconds,
      errors: ok ? [] : extractErrors(c.id, `${r.stdout ?? ''}\n${r.stderr ?? ''}${r.error ? `\n${r.error.message}` : ''}`, { root }),
    });
  }
  console.log(`\n=== ${title} summary ===`);
  for (const r of results) console.log(`${r.ok ? 'PASS' : 'FAIL'}  ${r.name}  (${r.seconds.toFixed(1)} s)`);
  const failed = results.filter((r) => !r.ok).length;
  console.log(failed ? `\n${failed} check(s) failed.` : '\nAll checks passed.');
  process.exit(failed ? 1 : 0);
}
