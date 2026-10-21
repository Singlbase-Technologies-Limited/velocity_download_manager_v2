#!/usr/bin/env node
// `pnpm check:links`: checks that relative links and heading anchors in every Markdown file git
// knows about (tracked, or untracked and not ignored) resolve. External links are not fetched.
// CI's docs job runs it (docs/CI.md). Logic and tests: links-lib.mjs.
import { spawnSync } from 'node:child_process';
import { existsSync, readFileSync, statSync } from 'node:fs';
import { join } from 'node:path';
import { brokenLinks } from './links-lib.mjs';
import { recordCheck, root } from './checks-runner.mjs';

const r = spawnSync('git', ['ls-files', '--cached', '--others', '--exclude-standard', '-z', '--', '*.md'], {
  cwd: root,
  encoding: 'utf8',
  windowsHide: true,
});
if (r.status !== 0) {
  console.error(`git ls-files failed:\n${r.stderr}`);
  process.exit(2);
}
const files = r.stdout.split('\0').filter(Boolean);
const read = (p) => {
  const abs = join(root, p);
  return existsSync(abs) && statSync(abs).isFile() ? readFileSync(abs, 'utf8') : undefined;
};
const isDir = (p) => {
  const abs = join(root, p);
  return existsSync(abs) && statSync(abs).isDirectory();
};

const broken = brokenLinks(files, read, isDir);
for (const b of broken) console.log(`BROKEN  ${b.file}: ${b.link} (${b.problem})`);
console.log(`check:links: ${files.length} Markdown file(s), ${broken.length} broken link(s).`);
recordCheck({
  id: 'links',
  name: 'Docs (check:links)',
  status: broken.length ? 'fail' : 'pass',
  errors: broken.slice(0, 10).map((x) => ({ file: x.file, message: `${x.link} (${x.problem})` })),
});
process.exit(broken.length ? 1 : 0);
