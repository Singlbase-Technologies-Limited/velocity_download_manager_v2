#!/usr/bin/env node
// The release workflow's steps (#18, docs/RELEASE_PROCESS.md). The rules are in release-lib.mjs;
// this file only reads and writes.
//
//   node scripts/release.mjs verify
//     For the pushed tag (GITHUB_REF_NAME, on GITHUB_SHA): refuses one that isn't
//     v<package.json version>, isn't on main, has no green Main checks run, or isn't on the commit
//     of a release candidate. Finds the accepted candidate's run and artifact, and writes them to
//     $GITHUB_OUTPUT. Needs GITHUB_TOKEN, GITHUB_REPOSITORY, and origin/main and the tags fetched.
//
//   node scripts/release.mjs assemble <candidate dir> <out dir>
//     Checks every file of the downloaded candidate against its SHA256SUMS, copies them to
//     <out dir> under the release's names, writes the release's SHA256SUMS there, and the release
//     notes from CHANGELOG.md to <out dir>/../notes.md.
//
// Tools run directly, never through a shell.
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { copyFileSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { root } from './checks-runner.mjs';
import { repoApi, writeOutputs } from './github-api.mjs';
import { acceptedCandidate, candidateArtifact, releaseFileName, releaseNotes, releaseRefusal } from './release-lib.mjs';
import { MAIN_CHECKS_WORKFLOW, mainChecksRun, sha256sums } from './release-candidate-lib.mjs';

const [command, ...args] = process.argv.slice(2);
const version = JSON.parse(readFileSync(join(root, 'package.json'), 'utf8')).version;

function git(...gitArgs) {
  const r = spawnSync('git', gitArgs, { cwd: root, encoding: 'utf8', windowsHide: true });
  if (r.error) throw r.error;
  return r;
}

function refuse(why) {
  console.error(`Refused: ${why}`);
  process.exit(1);
}

async function verify() {
  const tag = process.env.GITHUB_REF_TYPE === 'tag' ? process.env.GITHUB_REF_NAME : null;
  const commit = process.env.GITHUB_SHA;
  if (!tag || !commit) refuse('a release starts only from a pushed vX.Y.Z tag');
  const ancestor = git('merge-base', '--is-ancestor', commit, 'origin/main');
  if (ancestor.status > 1) throw new Error(`git merge-base failed: ${ancestor.stderr.trim()}`);
  const checks = await repoApi(`actions/workflows/${MAIN_CHECKS_WORKFLOW}/runs?head_sha=${commit}&per_page=100`);
  const mainChecks = mainChecksRun(checks.workflow_runs, commit);
  const tags = git('tag', '--points-at', commit).stdout.split('\n').filter(Boolean);
  const candidate = acceptedCandidate(tags, version);

  const why = releaseRefusal({ tag, version, commit, onMain: ancestor.status === 0, mainChecks, candidate });
  if (why) refuse(why);

  // The candidate's own run: the release-candidate workflow started by its tag, green.
  const runs = await repoApi(`actions/workflows/release-candidate.yml/runs?head_sha=${commit}&event=push&per_page=100`);
  const run = runs.workflow_runs.find((r) => r.head_branch === candidate && r.status === 'completed' && r.conclusion === 'success');
  if (!run) refuse(`${candidate} has no successful Release candidate run`);
  const artifacts = await repoApi(`actions/runs/${String(run.id)}/artifacts?per_page=100`);
  const artifact = candidateArtifact(artifacts.artifacts, candidate, commit);
  if (!artifact) refuse(`${candidate}'s files are gone (kept 14 days): build the candidate again with a new rc tag`);
  console.log(`Releasing ${tag}: ${candidate}, run ${run.html_url}`);
  await writeOutputs({ commit, candidate, run_id: run.id, artifact: artifact.name });
}

function sha256(path) {
  return createHash('sha256').update(readFileSync(path)).digest('hex');
}

function assemble(candidateDir, out) {
  if (!candidateDir || !out) throw new Error('usage: release.mjs assemble <candidate dir> <out dir>');
  // Every file must be the candidate's, byte for byte: what was tested is what is released.
  const listed = readFileSync(join(candidateDir, 'SHA256SUMS'), 'utf8')
    .split('\n')
    .filter(Boolean)
    .map((line) => {
      const m = /^([0-9a-f]{64}) {2}(.+)$/.exec(line);
      if (!m) throw new Error(`a line of SHA256SUMS isn't "<sha256>  <name>": ${line}`);
      return { sha256: m[1], name: m[2] };
    });
  for (const { sha256: want, name } of listed) {
    const got = sha256(join(candidateDir, name));
    if (got !== want) throw new Error(`${name} doesn't match the candidate's SHA256SUMS: ${got}, not ${want}`);
  }
  mkdirSync(out, { recursive: true });
  const released = listed.map(({ name, sha256: hash }) => {
    const to = name === 'build-info.json' ? `velocity-v2-${version}-build-info.json` : releaseFileName(name, version);
    copyFileSync(join(candidateDir, name), join(out, to));
    return { name: to, sha256: hash };
  });
  writeFileSync(join(out, 'SHA256SUMS'), sha256sums(released));
  writeFileSync(join(dirname(out), 'notes.md'), releaseNotes(readFileSync(join(root, 'CHANGELOG.md'), 'utf8'), version));
  console.log(`v${version}: ${String(released.length)} files, every one the candidate's\n${released.map((f) => `  ${f.name}`).join('\n')}\n  SHA256SUMS`);
}

if (command === 'verify') await verify();
else if (command === 'assemble') assemble(...args);
else {
  console.error('usage: release.mjs verify | assemble <candidate dir> <out dir>');
  process.exit(2);
}
