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
/** Directories whose change can affect any check (`scripts/` holds the check machinery). */
export const SELECT_ALL_PREFIXES = ['scripts/', '.cargo/'];
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
 * @returns {{all: boolean, frontend: boolean, rustPackages: string[], reasons: string[]}}
 */
export function selectChecks(changedPaths, workspace) {
  const reasons = [];
  let all = false;
  let frontend = false;
  const crates = new Set();

  for (const raw of changedPaths) {
    const p = normalise(raw);
    const crate = workspace.crates.find((c) => p.startsWith(`${c.dir}/`));
    if (SELECT_ALL_FILES.includes(p) || SELECT_ALL_PREFIXES.some((d) => p.startsWith(d))) {
      all = true;
      reasons.push(`${p}: shared file, selects everything`);
    } else if (crate) {
      crates.add(crate.name);
      reasons.push(`${p}: crate ${crate.name}`);
    } else if (FRONTEND_FILES.includes(p) || FRONTEND_PREFIXES.some((d) => p.startsWith(d))) {
      frontend = true;
      reasons.push(`${p}: frontend`);
    } else if (NO_CHECK_PREFIXES.some((d) => p.startsWith(d)) || isRootFileWithExtension(p, NO_CHECK_ROOT_EXTENSIONS)) {
      reasons.push(`${p}: documentation, no checks`);
    } else {
      all = true;
      reasons.push(`${p}: no rule for this path, selects everything`);
    }
  }

  if (all) return { all, frontend: true, rustPackages: workspace.crates.map((c) => c.name).sort(), reasons };
  return { all, frontend, rustPackages: withDependents([...crates], workspace), reasons };
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
        args: ['clippy', ...rust, '--all-targets', '--', '-D', 'warnings'],
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
