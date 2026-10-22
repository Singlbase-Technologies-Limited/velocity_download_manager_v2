// Pure logic behind the verification-level commands (`check:fast`, `check:pr`, `check:full`,
// `test:focused`): which checks a set of changed paths needs, and the commands for each check.
// No I/O here, so every rule is unit tested (checks-lib.test.mjs). The path rules are the single
// source the CI change detection (#13) should reuse.
//
// Safety rule: when in doubt, select everything. Lockfiles, shared configuration, the check
// scripts themselves and any path no rule knows select every check.

/** Paths whose change can affect any check. */
export const SELECT_ALL_FILES = [
  'Cargo.lock',
  'Cargo.toml',
  'pnpm-lock.yaml',
  'pnpm-workspace.yaml',
  'package.json',
  'rust-toolchain.toml',
  '.gitattributes',
  '.editorconfig',
  '.gitignore',
];
/** Directories whose change can affect any check (`scripts/` holds the check machinery, `.github/` the CI). */
export const SELECT_ALL_PREFIXES = ['scripts/', '.cargo/', '.github/'];
/**
 * Native dependency pins, wherever they live (for example a libcurl `deps.lock` or a vcpkg manifest,
 * #35): they change what every native build links, so they select everything even inside a crate.
 */
export const NATIVE_LOCK_FILE = /(^|\/)([^/]+\.lock|vcpkg\.json|vcpkg-configuration\.json)$/;
/** The native library builds (#35): their scripts, pins and keys. */
export const NATIVE_PREFIXES = ['scripts/native/'];
/** Lockfiles that match NATIVE_LOCK_FILE but pin Rust or npm packages, not native libraries. */
const PACKAGE_LOCK_FILES = ['Cargo.lock', 'pnpm-lock.yaml'];
/** Frontend: TypeScript types, ESLint and Vitest. */
export const FRONTEND_FILES = ['index.html', 'tsconfig.json', 'vite.config.ts', 'eslint.config.js'];
export const FRONTEND_PREFIXES = ['src/', 'public/'];
/** Documentation: no code check depends on it. */
export const NO_CHECK_PREFIXES = ['docs/'];
export const NO_CHECK_ROOT_EXTENSIONS = ['.md'];

const normalise = (p) => p.replaceAll('\\', '/').replace(/^\.\//, '');
const isRootFileWithExtension = (p, exts) => !p.includes('/') && exts.some((e) => p.toLowerCase().endsWith(e));

/** Workspace crates plus every crate that depends on them, directly or transitively. */
function withDependents(names, workspace) {
  const selected = new Set(names);
  let grew = true;
  while (grew) {
    grew = false;
    for (const c of workspace.crates) {
      if (!selected.has(c.name) && c.deps.some((d) => selected.has(d))) {
        selected.add(c.name);
        grew = true;
      }
    }
  }
  return [...selected].sort();
}

/**
 * @param {string[]} changedPaths repository-relative paths (added, modified, deleted or renamed)
 * @param {{crates: {name: string, dir: string, deps: string[]}[]}} workspace
 * @returns {{all: boolean, frontend: boolean, docs: boolean, rustPackages: string[], reasons: string[]}}
 */
export function selectChecks(changedPaths, workspace) {
  const reasons = [];
  let all = false;
  let frontend = false;
  let docs = false;
  const crates = new Set();

  for (const raw of changedPaths) {
    const p = normalise(raw);
    const crate = workspace.crates.find((c) => p.startsWith(`${c.dir}/`));
    if (SELECT_ALL_FILES.includes(p) || SELECT_ALL_PREFIXES.some((d) => p.startsWith(d)) || NATIVE_LOCK_FILE.test(p)) {
      all = true;
      reasons.push(`${p}: shared file, selects everything`);
    } else if (crate) {
      crates.add(crate.name);
      reasons.push(`${p}: crate ${crate.name}`);
    } else if (FRONTEND_FILES.includes(p) || FRONTEND_PREFIXES.some((d) => p.startsWith(d))) {
      frontend = true;
      reasons.push(`${p}: frontend`);
    } else if (NO_CHECK_PREFIXES.some((d) => p.startsWith(d)) || isRootFileWithExtension(p, NO_CHECK_ROOT_EXTENSIONS)) {
      docs = true;
      reasons.push(`${p}: documentation, no code checks`);
    } else {
      all = true;
      reasons.push(`${p}: no rule for this path, selects everything`);
    }
  }

  if (all) return { all, frontend: true, docs: true, rustPackages: workspace.crates.map((c) => c.name).sort(), reasons };
  return { all, frontend, docs, rustPackages: withDependents([...crates], workspace), reasons };
}

/**
 * CI job selection (#13, docs/CI.md): which pull-request jobs a change needs, each with the reason
 * it runs or is skipped, so the run summary can show both. Built on `selectChecks`, so local
 * `check:pr` and CI can't disagree.
 * @returns {{all: boolean, rustPackages: string[], reasons: string[],
 *   jobs: {rust: {run: boolean, reason: string}, frontend: {run: boolean, reason: string}, docs: {run: boolean, reason: string},
 *     native: {run: boolean, reason: string}}}}
 */
export function selectJobs(changedPaths, workspace) {
  const s = selectChecks(changedPaths, workspace);
  const paths = changedPaths.map(normalise);
  const forcing = s.reasons.filter((r) => r.includes('selects everything')).map((r) => r.slice(0, r.indexOf(':')));
  const allReason = `everything selected by ${forcing.join(', ')}`;
  const job = (run, runReason, skipReason) => ({ run, reason: !paths.length ? 'no changed paths' : s.all ? allReason : run ? runReason : skipReason });
  const docsPaths = s.reasons.filter((r) => r.endsWith('documentation, no code checks')).map((r) => r.slice(0, r.indexOf(':')));
  // The native builds take minutes (Windows bills double), so a shared file doesn't select them:
  // only their own inputs, CI files and paths no rule knows.
  const unknown = new Set(s.reasons.filter((r) => r.endsWith('no rule for this path, selects everything')).map((r) => r.slice(0, r.indexOf(':'))));
  const nativePaths = paths.filter(
    (p) =>
      NATIVE_PREFIXES.some((d) => p.startsWith(d)) ||
      p.startsWith('.github/') ||
      unknown.has(p) ||
      (NATIVE_LOCK_FILE.test(p) && !PACKAGE_LOCK_FILES.includes(p.slice(p.lastIndexOf('/') + 1))),
  );
  return {
    all: s.all,
    rustPackages: s.rustPackages,
    reasons: s.reasons,
    jobs: {
      rust: job(s.rustPackages.length > 0, `crates changed: ${s.rustPackages.join(', ')}`, 'no Rust crate changed'),
      frontend: job(s.frontend, 'frontend files changed', 'no frontend file changed'),
      docs: job(s.docs, `documentation changed: ${docsPaths.join(', ')}`, 'no documentation changed'),
      native: {
        run: nativePaths.length > 0,
        reason: !paths.length ? 'no changed paths' : nativePaths.length ? `native build inputs changed: ${nativePaths.join(', ')}` : 'no native build input changed',
      },
    },
  };
}

/** The run-summary table for `selectJobs` (GitHub's step summary): every job, run or skipped, with why. */
export function jobSummaryMarkdown(selection) {
  const cell = (s) => String(s).replaceAll('|', '\\|');
  const rows = Object.entries(selection.jobs).map(([name, j]) => `| ${name} | ${j.run ? 'runs' : 'skipped'} | ${cell(j.reason)} |`);
  const paths = selection.reasons.length ? selection.reasons.map((r) => `- ${cell(r)}`) : ['- (none)'];
  return ['### Job selection', '', '| Job | Decision | Reason |', '| --- | --- | --- |', ...rows, '', '**Changed paths**', '', ...paths, ''].join('\n');
}

/** Builds the workspace description from `cargo metadata --format-version 1 --no-deps`. */
export function workspaceFromMetadata(metadata) {
  const root = normalise(metadata.workspace_root).replace(/\/$/, '');
  const names = new Set(metadata.packages.map((p) => p.name));
  return {
    crates: metadata.packages.map((p) => ({
      name: p.name,
      dir: normalise(p.manifest_path).slice(root.length + 1).replace(/\/?Cargo\.toml$/, ''),
      deps: p.dependencies.filter((d) => d.path !== undefined && names.has(d.name)).map((d) => d.name),
    })),
  };
}

const pkgArgs = (pkgs) => pkgs.flatMap((p) => ['-p', p]);

/**
 * The L1 checks for a selection. `tool` is resolved to an executable by the runner.
 * @returns {{id: string, name: string, tool: string, args: string[]}[]}
 */
export function checkCommands(selection) {
  const cmds = [];
  const rust = selection.all ? ['--workspace'] : pkgArgs(selection.rustPackages);
  if (selection.all || selection.rustPackages.length > 0) {
    cmds.push(
      { id: 'rust-fmt', name: 'Rust formatting (cargo fmt --check)', tool: 'cargo', args: ['fmt', '--all', '--', '--check'] },
      {
        id: 'rust-clippy',
        name: 'Rust lint (cargo clippy -D warnings)',
        tool: 'cargo',
        args: ['clippy', ...rust, '--all-targets', '--keep-going', '--', '-D', 'warnings'],
      },
      { id: 'rust-test', name: 'Rust unit tests (cargo test)', tool: 'cargo', args: ['test', ...rust, '--no-fail-fast'] },
    );
  }
  if (selection.all || selection.frontend) {
    cmds.push(
      { id: 'tsc', name: 'TypeScript types (tsc --noEmit)', tool: 'tsc', args: ['--noEmit'] },
      { id: 'eslint', name: 'ESLint (zero warnings)', tool: 'eslint', args: ['.', '--max-warnings', '0'] },
      { id: 'vitest', name: 'Vitest unit tests', tool: 'vitest', args: ['run'] },
    );
  }
  return cmds;
}

/** Extra checks that `check:full` runs after every L1 check. */
export const FULL_EXTRA_COMMANDS = [
  { id: 'web-build', name: 'Frontend production build (vite build)', tool: 'vite', args: ['build'] },
  { id: 'rust-release', name: 'Rust release build (cargo build --release)', tool: 'cargo', args: ['build', '--workspace', '--release'] },
];

const FRONTEND_AREAS = { frontend: 'src', scripts: 'scripts' };

/**
 * `test:focused -- <area> [extra args]`: the tests of one area. A crate area runs
 * `cargo test -p <crate> [extra]` (unit and integration tests); `frontend` and `scripts` run Vitest
 * on that directory.
 */
export function focusedCommands(argv, workspace) {
  const args = argv[0] === '--' ? argv.slice(1) : argv;
  const [area, ...extra] = args;
  const valid = [...workspace.crates.map((c) => c.name), ...Object.keys(FRONTEND_AREAS)];
  if (!area) throw new Error(`Name an area to test: ${valid.join(', ')}`);
  if (workspace.crates.some((c) => c.name === area)) {
    return [{ id: 'rust-test', tool: 'cargo', args: ['test', '-p', area, ...extra], name: `cargo test -p ${area}` }];
  }
  const dir = FRONTEND_AREAS[area];
  if (dir) return [{ id: 'vitest', tool: 'vitest', args: ['run', dir, ...extra], name: `vitest run ${dir}` }];
  throw new Error(`Unknown area "${area}". Valid areas: ${valid.join(', ')}`);
}
