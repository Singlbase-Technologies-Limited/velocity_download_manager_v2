// Pure logic behind the release-candidate workflow (#17, docs/RELEASE_CANDIDATES.md): which
// commits may become one, what its files are called, and the records that go with them. No I/O
// here, so every rule is unit tested (release-candidate-lib.test.mjs).

/** A release-candidate tag: `vX.Y.Z-rc.N` (docs/DEVELOPMENT_WORKFLOW.md, "Releases"). */
const RC_TAG = /^v(\d+\.\d+\.\d+)-rc\.([1-9]\d*)$/;

/** The workflow whose green run on `main` a commit needs (docs/CI.md, "The post-merge tier"). */
export const MAIN_CHECKS_WORKFLOW = 'main.yml';

/** `vX.Y.Z-rc.N` as `{ version, rc }`, or null for any other name. */
export function parseRcTag(tag) {
  const m = RC_TAG.exec(tag);
  return m ? { version: m[1], rc: Number(m[2]) } : null;
}

/**
 * Why `commit` can't become a release candidate, or null when it can.
 * - `onMain`: whether it is on `main` (`git merge-base --is-ancestor <commit> origin/main`).
 * - `mainChecks`: the green Main checks run on it, or null.
 * - `tag`: the tag that started the run, if one did; `version`: package.json's.
 */
export function refusal({ commit, onMain, mainChecks, tag, version }) {
  if (!onMain) return `${commit} is not on main: release candidates come only from main (#17)`;
  if (!mainChecks) return `${commit} has no green Main checks run on main: run them first (docs/CI.md)`;
  if (tag !== undefined && tag !== null) {
    const parsed = parseRcTag(tag);
    if (!parsed) return `${tag} is not a release-candidate tag (vX.Y.Z-rc.N)`;
    if (parsed.version !== version) return `${tag} is for ${parsed.version}, but package.json says ${version}`;
  }
  return null;
}

/**
 * The green Main checks run for `commit` on `main`, from the API's list of that workflow's runs
 * (`GET /repos/{repo}/actions/workflows/main.yml/runs?head_sha=…`), or null.
 */
export function mainChecksRun(runs, commit) {
  return runs.find((r) => r.head_sha === commit && r.head_branch === 'main' && r.status === 'completed' && r.conclusion === 'success') ?? null;
}

/** `YYYYMMDD` for a date, in UTC. */
export function compactDate(date) {
  return date.toISOString().slice(0, 10).replaceAll('-', '');
}

/**
 * The name every file of the candidate starts with:
 * `velocity-v2-<version>-<rc.N, or main>-<short commit>-<YYYYMMDD>`.
 */
export function candidateName({ version, tag, commit, date }) {
  const parsed = tag ? parseRcTag(tag) : null;
  const label = parsed ? `rc.${parsed.rc}` : 'main';
  return `velocity-v2-${version}-${label}-${commit.slice(0, 7)}-${compactDate(date)}`;
}

/** The packages a candidate carries: where each comes from in the packaging artifacts, and its suffix. */
export const PACKAGES = [
  { platform: 'windows', match: /-setup\.exe$/, suffix: 'windows-x64-setup.exe' },
  { platform: 'linux', match: /\.deb$/, suffix: 'linux-amd64.deb' },
  { platform: 'linux', match: /\.rpm$/, suffix: 'linux-x86_64.rpm' },
];

/**
 * The candidate's file name for each package in `files` (names as the packaging workflows made
 * them), as `{ from, to }`. Throws unless each package is there exactly once.
 */
export function packageNames(name, files) {
  return PACKAGES.map(({ match, suffix }) => {
    const found = files.filter((f) => match.test(f));
    if (found.length !== 1) throw new Error(`expected one ${suffix} package, found: ${found.join(', ') || 'none'}`);
    return { from: found[0], to: `${name}-${suffix}` };
  });
}

/** `SHA256SUMS` in the `sha256sum` format, sorted by file name: `<hash>  <name>` lines. */
export function sha256sums(entries) {
  return entries
    .slice()
    .sort((a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0))
    .map((e) => `${e.sha256}  ${e.name}\n`)
    .join('');
}

/**
 * The candidate's `build-info.json`: what it is, where it was built, and each platform's native
 * libraries and toolchains as their own records say (docs/BUILDING_NATIVE.md, `build-info.json`).
 */
export function candidateBuildInfo({ name, version, tag, commit, date, run, files, platforms }) {
  return {
    schema: 1,
    name,
    version,
    tag: tag ?? null,
    commit,
    built: date.toISOString(),
    run,
    files,
    platforms,
  };
}

/**
 * Job-level conditions that let a job run after a job it needs failed or was cancelled: `always()`,
 * `!cancelled()`, `failure()` and `cancelled()` in a job's `if:`, and `continue-on-error` anywhere.
 * A packaging or release-candidate workflow must have none (#17, from #91): its jobs run only when
 * every job they need succeeded. `workflow` is the YAML text; jobs sit at two spaces under `jobs:`,
 * their keys at four. Returns the problems, each naming its line.
 */
export function strictNeedsProblems(workflow) {
  const problems = [];
  let inJobs = false;
  workflow.split(/\r?\n/).forEach((line, i) => {
    if (/^jobs:\s*$/.test(line)) inJobs = true;
    else if (/^\S/.test(line)) inJobs = false;
    if (/^\s*continue-on-error:/.test(line)) problems.push(`line ${i + 1}: continue-on-error lets the workflow pass with a failed step`);
    if (inJobs && /^ {4}if:/.test(line) && /\b(always|failure|cancelled)\(\)/.test(line)) {
      problems.push(`line ${i + 1}: a job condition that runs after a failed or cancelled job: ${line.trim()}`);
    }
  });
  return problems;
}
