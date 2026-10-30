#!/usr/bin/env node
// `pnpm changelog <from> [<to>] [--heading "<heading>"]` (#84, docs/RELEASE_PROCESS.md): prints a
// draft CHANGELOG.md section for the commits on main's first-parent line after <from> up to <to>
// (origin/main by default), grouped by their Conventional Commit types. <from> is a tag such as
// v0.1.0, a commit, or `start` for the first commit. The heading is `Unreleased` by default.
// Subjects that aren't Conventional Commits are listed on stderr; the draft keeps them as they are.
import { spawnSync } from 'node:child_process';
import { root } from './checks-runner.mjs';
import { changelogSection } from './changelog-lib.mjs';

function git(...args) {
  const r = spawnSync('git', args, { cwd: root, encoding: 'utf8', windowsHide: true, maxBuffer: 64 * 1024 * 1024 });
  if (r.error) throw r.error;
  if (r.status !== 0) throw new Error(`git ${args.join(' ')} failed: ${r.stderr.trim()}`);
  return r.stdout;
}

const args = process.argv.slice(2);
const at = args.indexOf('--heading');
const heading = at === -1 ? 'Unreleased' : args[at + 1];
const [from, to = 'origin/main'] = at === -1 ? args : args.filter((_, i) => i !== at && i !== at + 1);
if (!from || !heading) {
  console.error('usage: pnpm changelog <from: tag, commit or start> [<to>] [--heading "<heading>"]');
  process.exit(2);
}

const range = from === 'start' ? to : `${from}..${to}`;
const subjects = git('log', '--first-parent', '--format=%s', range).split('\n').filter(Boolean);
const { text, unparsed } = changelogSection(heading, subjects);
process.stdout.write(text);
if (unparsed.length) {
  console.error(`\n${unparsed.length} subject(s) aren't Conventional Commits; they are under Other changes as they are:\n${unparsed.map((s) => `- ${s}`).join('\n')}`);
}
