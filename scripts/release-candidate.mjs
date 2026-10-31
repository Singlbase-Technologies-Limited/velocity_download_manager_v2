#!/usr/bin/env node
// The release-candidate workflow's two steps (#17, docs/RELEASE_CANDIDATES.md). The rules are in
// release-candidate-lib.mjs; this file only reads and writes.
//
//   node scripts/release-candidate.mjs verify <commit>
//     Refuses a commit that isn't on main, has no green Main checks run on main, or was tagged with
//     a tag that isn't `v<package.json version>-rc.N`. Prints the commit and the candidate's name,
//     and writes them to $GITHUB_OUTPUT in CI. Needs GITHUB_TOKEN and GITHUB_REPOSITORY, and
//     origin/main fetched. The tag is GITHUB_REF_NAME when GITHUB_REF_TYPE is `tag`.
//
//   node scripts/release-candidate.mjs assemble <packages dir> <out dir> <commit> <name>
//     Copies each package from the packaging artifacts under <packages dir> to <out dir> under the
//     candidate's name, with the platforms' licence reports and native records, and writes
//     SHA256SUMS and build-info.json there.
//
// Tools run directly, never through a shell.
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { copyFileSync, mkdirSync, readdirSync, readFileSync, statSync, writeFileSync } from 'node:fs';
import { basename, join } from 'node:path';
import { root } from './checks-runner.mjs';
import { repoApi, writeOutputs } from './github-api.mjs';
import { candidateBuildInfo, candidateName, MAIN_CHECKS_WORKFLOW, mainChecksRun, packageNames, refusal, sha256sums } from './release-candidate-lib.mjs';

const [command, ...args] = process.argv.slice(2);
const version = JSON.parse(readFileSync(join(root, 'package.json'), 'utf8')).version;
const tag = process.env.GITHUB_REF_TYPE === 'tag' ? process.env.GITHUB_REF_NAME : null;

function git(...gitArgs) {
  const r = spawnSync('git', gitArgs, { cwd: root, encoding: 'utf8', windowsHide: true });
  if (r.error) throw r.error;
  return r;
}

async function verify(ref) {
  if (!ref) throw new Error('usage: release-candidate.mjs verify <commit>');
  const parsed = git('rev-parse', '--verify', `${ref}^{commit}`);
  if (parsed.status !== 0) throw new Error(`${ref} is not a commit here: ${parsed.stderr.trim()}`);
  const commit = parsed.stdout.trim();
  const ancestor = git('merge-base', '--is-ancestor', commit, 'origin/main');
  if (ancestor.status > 1) throw new Error(`git merge-base failed: ${ancestor.stderr.trim()}`);

  const runs = await repoApi(`actions/workflows/${MAIN_CHECKS_WORKFLOW}/runs?head_sha=${commit}&per_page=100`);
  const mainChecks = mainChecksRun(runs.workflow_runs, commit);

  const why = refusal({ commit, onMain: ancestor.status === 0, mainChecks, tag, version });
  if (why) {
    console.error(`Refused: ${why}`);
    process.exit(1);
  }
  console.log(`Main checks: ${mainChecks.html_url}`);
  await writeOutputs({ commit, name: candidateName({ version, tag, commit, date: new Date() }) });
}

/** Every file under `dir`, as paths. */
function walk(dir) {
  return readdirSync(dir, { withFileTypes: true }).flatMap((e) => (e.isDirectory() ? walk(join(dir, e.name)) : [join(dir, e.name)]));
}

function sha256(path) {
  return createHash('sha256').update(readFileSync(path)).digest('hex');
}

function assemble(packages, out, commit, name) {
  if (!packages || !out || !commit || !name) throw new Error('usage: release-candidate.mjs assemble <packages dir> <out dir> <commit> <name>');
  mkdirSync(out, { recursive: true });
  const files = walk(packages);
  const copied = [];
  for (const { from, to } of packageNames(name, files.map((f) => basename(f)))) {
    copyFileSync(files.find((f) => basename(f) === from), join(out, to));
    copied.push(to);
  }
  // Each packaging artifact carries its platform's records (scripts/package-build.mjs).
  const platforms = {};
  for (const platform of ['windows', 'linux']) {
    const record = (file) => {
      const path = files.find((f) => basename(f) === file);
      if (!path) throw new Error(`the ${platform} packages came without ${file}`);
      return path;
    };
    const report = `licence-report-${platform}.json`;
    copyFileSync(record(report), join(out, `${name}-${report}`));
    copied.push(`${name}-${report}`);
    platforms[platform] = {
      native: JSON.parse(readFileSync(record(`native-build-info-${platform}.json`), 'utf8')),
      libtorrent: JSON.parse(readFileSync(record(`libtorrent-info-${platform}.json`), 'utf8')),
    };
  }
  const entries = copied.map((f) => ({ name: f, sha256: sha256(join(out, f)), bytes: statSync(join(out, f)).size }));
  const server = process.env.GITHUB_SERVER_URL ?? 'https://github.com';
  const run = process.env.GITHUB_RUN_ID ? { id: Number(process.env.GITHUB_RUN_ID), url: `${server}/${process.env.GITHUB_REPOSITORY}/actions/runs/${process.env.GITHUB_RUN_ID}` } : null;
  const info = candidateBuildInfo({ name, version, tag, commit, date: new Date(), run, files: entries, platforms });
  writeFileSync(join(out, 'build-info.json'), `${JSON.stringify(info, null, 2)}\n`);
  const sums = [...entries, { name: 'build-info.json', sha256: sha256(join(out, 'build-info.json')) }];
  writeFileSync(join(out, 'SHA256SUMS'), sha256sums(sums));
  console.log(`${name}:\n${sums.map((e) => `  ${e.name}`).join('\n')}\n  SHA256SUMS`);
}

if (command === 'verify') await verify(args[0]);
else if (command === 'assemble') assemble(...args);
else {
  console.error('usage: release-candidate.mjs verify <commit> | assemble <packages dir> <out dir> <commit> <name>');
  process.exit(2);
}
