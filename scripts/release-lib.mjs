// Pure logic behind the release workflow (#18, docs/RELEASE_PROCESS.md): which tag may become a
// release, which release candidate it publishes, its files' names and its notes. No I/O here, so
// every rule is unit tested (release-lib.test.mjs). A commit's place on main and its Main checks
// are release-candidate-lib.mjs's rules, shared.
import { parseRcTag, refusal } from './release-candidate-lib.mjs';

/** A release tag: `vX.Y.Z`, never a pre-release (docs/DEVELOPMENT_WORKFLOW.md, "Releases"). */
const RELEASE_TAG = /^v(\d+\.\d+\.\d+)$/;

/** The version of a release tag, or null for any other name. */
export function parseReleaseTag(tag) {
  return RELEASE_TAG.exec(tag)?.[1] ?? null;
}

/**
 * The accepted release candidate a release publishes: of the tags on its commit, the
 * `v<version>-rc.N` with the highest N, or null when there is none. A release is a tag on the
 * commit of the candidate the PO accepted (docs/DEVELOPMENT_WORKFLOW.md), so its installers are
 * that candidate's, the ones that were tested.
 */
export function acceptedCandidate(tagsOnCommit, version) {
  const candidates = tagsOnCommit
    .map((tag) => ({ tag, rc: parseRcTag(tag) }))
    .filter(({ rc }) => rc !== null && rc.version === version)
    .sort((a, b) => b.rc.rc - a.rc.rc);
  return candidates[0]?.tag ?? null;
}

/**
 * Why `tag` on `commit` can't be released, or null when it can: it must be `v<package.json
 * version>`, on main with green Main checks, and on the commit of a release candidate.
 */
export function releaseRefusal({ tag, version, commit, onMain, mainChecks, candidate }) {
  const released = parseReleaseTag(tag);
  if (released === null) return `${tag} is not a release tag (vX.Y.Z)`;
  if (released !== version) return `${tag} is for ${released}, but package.json says ${version}`;
  const notReady = refusal({ commit, onMain, mainChecks });
  if (notReady) return notReady;
  if (candidate === null) return `${commit} has no release-candidate tag v${version}-rc.N: release the commit of an accepted candidate (docs/RELEASE_CANDIDATES.md)`;
  return null;
}

/**
 * The candidate's artifact among a run's artifacts (`GET …/actions/runs/{id}/artifacts`): the one
 * named `velocity-v2-<version>-rc.<N>-<short commit>-…`, or null.
 */
export function candidateArtifact(artifacts, candidate, commit) {
  const rc = parseRcTag(candidate);
  if (rc === null) return null;
  const prefix = `velocity-v2-${rc.version}-rc.${String(rc.rc)}-${commit.slice(0, 7)}-`;
  return artifacts.find((a) => a.name.startsWith(prefix) && !a.expired) ?? null;
}

/**
 * The release's name for a candidate's file: the candidate's prefix
 * (`velocity-v2-<version>-rc.<N>-<commit>-<date>-`) becomes `velocity-v2-<version>-`, so
 * `…-windows-x64-setup.exe` stays `…-windows-x64-setup.exe`. The bytes are the candidate's.
 */
export function releaseFileName(candidateFile, version) {
  const m = /^velocity-v2-\d+\.\d+\.\d+-rc\.\d+-[0-9a-f]{7}-\d{8}-(.+)$/.exec(candidateFile);
  if (!m) throw new Error(`${candidateFile} isn't a release candidate's file`);
  return `velocity-v2-${version}-${m[1]}`;
}

/**
 * The release notes: the section of CHANGELOG.md for `version`, whose heading is `## <version>`
 * followed by anything, such as ` - 2026-11-02`, up to the next `## `. Throws when there is none:
 * a release needs its notes written first (docs/RELEASE_PROCESS.md).
 */
export function releaseNotes(changelog, version) {
  const lines = changelog.split(/\r?\n/);
  const start = lines.findIndex((l) => l === `## ${version}` || l.startsWith(`## ${version} `));
  if (start === -1) throw new Error(`CHANGELOG.md has no "## ${version}" section: write the release notes first (docs/RELEASE_PROCESS.md)`);
  const end = lines.findIndex((l, i) => i > start && l.startsWith('## '));
  const body = lines.slice(start + 1, end === -1 ? undefined : end).join('\n').trim();
  if (!body) throw new Error(`CHANGELOG.md's "## ${version}" section is empty`);
  return `${body}\n`;
}
