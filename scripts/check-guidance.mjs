#!/usr/bin/env node
// `pnpm check:guidance` (#88, docs/AGENT_GUIDANCE.md): checks AGENTS.md, CLAUDE.md, the
// path-scoped rules and the skills for drift that Claude Code would not report (the checks are in
// guidance-lib.mjs, unit tested). Prints every problem and exits 1 if there is any.
import { spawnSync } from 'node:child_process';
import { existsSync, readFileSync, readdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { recordCheck } from './checks-runner.mjs';
import { checkGuidance } from './guidance-lib.mjs';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');

/** Every guidance file Claude Code loads from this repository, by repository-relative path. */
export function loadGuidance() {
  const files = {};
  const read = (rel) => (files[rel] = readFileSync(join(root, rel), 'utf8'));
  for (const f of ['AGENTS.md', 'CLAUDE.md']) read(f);
  for (const f of readdirSync(join(root, '.claude', 'rules')).filter((n) => n.endsWith('.md')).sort()) read(`.claude/rules/${f}`);
  for (const d of readdirSync(join(root, '.claude', 'skills')).sort()) {
    if (existsSync(join(root, '.claude', 'skills', d, 'SKILL.md'))) read(`.claude/skills/${d}/SKILL.md`);
  }
  // Tracked and new (not ignored) files: what the rule globs should match.
  const ls = spawnSync('git', ['ls-files', '--cached', '--others', '--exclude-standard'], { cwd: root, encoding: 'utf8', maxBuffer: 64 * 1024 * 1024 });
  if (ls.status !== 0) throw new Error(`git ls-files failed: ${ls.stderr}`);
  const tracked = ls.stdout.split('\n').filter(Boolean);
  const scripts = Object.keys(JSON.parse(readFileSync(join(root, 'package.json'), 'utf8')).scripts ?? {});
  return { files, tracked, scripts };
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  const started = Date.now();
  let problems;
  try {
    const input = loadGuidance();
    problems = checkGuidance(input);
    console.log(`check:guidance: ${Object.keys(input.files).length} guidance file(s), ${problems.length} problem(s), ${Date.now() - started} ms.`);
  } catch (e) {
    console.error(`check:guidance failed: ${e instanceof Error ? e.message : e}`);
    process.exit(1);
  }
  for (const p of problems) console.log(`- ${p}`);
  recordCheck({
    id: 'guidance',
    name: 'Agent guidance (check:guidance)',
    status: problems.length ? 'fail' : 'pass',
    errors: problems.slice(0, 10).map((message) => ({ message })),
  });
  process.exit(problems.length ? 1 : 0);
}
