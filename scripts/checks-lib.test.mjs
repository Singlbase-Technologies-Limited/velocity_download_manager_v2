import { describe, expect, it } from 'vitest';
import { checkCommands, focusedCommands, jobSummaryMarkdown, selectChecks, selectJobs, workspaceFromMetadata } from './checks-lib.mjs';

// Mirrors this repository: the desktop shell depends on the core crate.
const workspace = {
  crates: [
    { name: 'velocity-core', dir: 'crates/velocity-core', deps: [] },
    { name: 'velocity-desktop-v2', dir: 'src-tauri', deps: ['velocity-core'] },
  ],
};

describe('selectChecks', () => {
  it('selects only the frontend checks for a frontend-only change', () => {
    const s = selectChecks(['src/App.tsx', 'src/appInfo.test.ts'], workspace);
    expect(s).toMatchObject({ all: false, frontend: true, rustPackages: [] });
  });

  it('selects one crate for a change to a crate nothing else depends on', () => {
    const s = selectChecks(['src-tauri/src/lib.rs'], workspace);
    expect(s).toMatchObject({ all: false, frontend: false, rustPackages: ['velocity-desktop-v2'] });
  });

  it('also selects the crates that depend on a changed crate', () => {
    const s = selectChecks(['crates/velocity-core/src/lib.rs'], workspace);
    expect(s.rustPackages).toEqual(['velocity-core', 'velocity-desktop-v2']);
    expect(s.all).toBe(false);
  });

  it('selects everything for a lockfile change', () => {
    expect(selectChecks(['Cargo.lock'], workspace).all).toBe(true);
    expect(selectChecks(['pnpm-lock.yaml'], workspace).all).toBe(true);
  });

  it('selects everything for shared configuration', () => {
    for (const p of ['Cargo.toml', 'package.json', 'rust-toolchain.toml', '.gitattributes']) {
      expect(selectChecks([p], workspace).all, p).toBe(true);
    }
  });

  it('selects everything for a change to the check scripts themselves', () => {
    expect(selectChecks(['scripts/checks-lib.mjs'], workspace).all).toBe(true);
  });

  it('selects everything for an unknown path, and says why', () => {
    const s = selectChecks(['docs/x.md', 'some/new/place.txt'], workspace);
    expect(s.all).toBe(true);
    expect(s.reasons.join('\n')).toContain('some/new/place.txt');
  });

  it('selects nothing for documentation-only changes', () => {
    const s = selectChecks(['docs/TESTING.md', 'README.md', 'docs/images/a.png'], workspace);
    expect(s).toMatchObject({ all: false, frontend: false, rustPackages: [] });
  });

  it('selects nothing for an empty change', () => {
    expect(selectChecks([], workspace)).toMatchObject({ all: false, frontend: false, rustPackages: [] });
  });

  it('does not treat src-tauri/ as frontend src/', () => {
    expect(selectChecks(['src-tauri/tauri.conf.json'], workspace).frontend).toBe(false);
  });

  it('accepts Windows separators', () => {
    expect(selectChecks(['crates\\velocity-core\\src\\lib.rs'], workspace).rustPackages).toContain('velocity-core');
  });
});

describe('workspaceFromMetadata', () => {
  it('reads member crates, their directories and their workspace dependencies', () => {
    const root = '/repo';
    const metadata = {
      workspace_root: root,
      packages: [
        {
          name: 'velocity-core',
          manifest_path: '/repo/crates/velocity-core/Cargo.toml',
          dependencies: [{ name: 'serde' }],
        },
        {
          name: 'velocity-desktop-v2',
          manifest_path: '/repo/src-tauri/Cargo.toml',
          dependencies: [{ name: 'tauri' }, { name: 'velocity-core', path: '/repo/crates/velocity-core' }],
        },
      ],
    };
    expect(workspaceFromMetadata(metadata)).toEqual(workspace);
  });

  it('handles Windows manifest paths', () => {
    const metadata = {
      workspace_root: 'C:\\repo',
      packages: [{ name: 'velocity-core', manifest_path: 'C:\\repo\\crates\\velocity-core\\Cargo.toml', dependencies: [] }],
    };
    expect(workspaceFromMetadata(metadata).crates[0]?.dir).toBe('crates/velocity-core');
  });
});

describe('checkCommands', () => {
  const ids = (sel) => checkCommands(sel).map((c) => c.id);

  it('runs every L1 check when everything is selected', () => {
    expect(ids({ all: true, frontend: false, rustPackages: [] })).toEqual([
      'native-build',
      'rust-fmt',
      'rust-clippy',
      'rust-test',
      'tsc',
      'eslint',
      'vitest',
    ]);
  });

  it('limits clippy and tests to the selected packages', () => {
    const cmds = checkCommands({ all: false, frontend: false, rustPackages: ['velocity-core'] });
    expect(cmds.map((c) => c.id)).toEqual(['native-build', 'rust-fmt', 'rust-clippy', 'rust-test']);
    expect(cmds.find((c) => c.id === 'rust-test')?.args).toEqual(['test', '-p', 'velocity-core', '--no-fail-fast']);
  });

  it('keeps clippy going past a crate that fails to compile, so every crate reports (#91)', () => {
    const clippy = checkCommands({ all: true, frontend: false, rustPackages: [] }).find((c) => c.id === 'rust-clippy');
    expect(clippy?.args.slice(0, clippy.args.indexOf('--'))).toContain('--keep-going');
  });

  // Every Rust build links the Velocity-built libcurl (#35), so the native build (reused in about a
  // second when up to date) runs before any Rust check.
  it('builds the native libraries before the Rust checks', () => {
    expect(checkCommands({ all: false, frontend: false, rustPackages: ['velocity-curl'] })[0]).toEqual({
      id: 'native-build',
      name: 'Native libraries (native:build)',
      tool: 'node',
      args: ['scripts/native/build.mjs'],
    });
  });

  it('runs no Rust checks for a frontend-only selection', () => {
    expect(ids({ all: false, frontend: true, rustPackages: [] })).toEqual(['tsc', 'eslint', 'vitest']);
  });

  it('runs nothing for an empty selection', () => {
    expect(checkCommands({ all: false, frontend: false, rustPackages: [] })).toEqual([]);
  });
});

describe('focusedCommands', () => {
  it('runs one crate with its tests and passes extra arguments to cargo test', () => {
    expect(focusedCommands(['velocity-core', 'range_probe'], workspace)).toEqual([
      { id: 'native-build', name: 'Native libraries (native:build)', tool: 'node', args: ['scripts/native/build.mjs'] },
      { id: 'rust-test', tool: 'cargo', args: ['test', '-p', 'velocity-core', 'range_probe'], name: 'cargo test -p velocity-core' },
    ]);
  });

  it('accepts a leading -- from the package manager', () => {
    expect(focusedCommands(['--', 'velocity-core'], workspace).at(-1)?.args).toEqual(['test', '-p', 'velocity-core']);
  });

  it('runs the frontend or script tests with Vitest', () => {
    expect(focusedCommands(['frontend'], workspace)[0]).toMatchObject({ tool: 'vitest', args: ['run', 'src'] });
    expect(focusedCommands(['scripts', '-t', 'selectChecks'], workspace)[0]).toMatchObject({
      tool: 'vitest',
      args: ['run', 'scripts', '-t', 'selectChecks'],
    });
  });

  it('rejects an unknown or missing area with the list of valid ones', () => {
    expect(() => focusedCommands(['nope'], workspace)).toThrow(/velocity-core.*velocity-desktop-v2.*frontend.*scripts/);
    expect(() => focusedCommands([], workspace)).toThrow(/area/);
  });
});

describe('selectJobs (CI, #13)', () => {
  // [case, changed paths, expected jobs, expected Rust packages]
  const all = ['velocity-core', 'velocity-desktop-v2'];
  const cases = [
    ['docs only', ['docs/CI.md', 'README.md'], { rust: false, frontend: false, docs: true }, []],
    ['frontend only', ['src/App.tsx'], { rust: false, frontend: true, docs: false }, []],
    ['one crate with a dependent', ['crates/velocity-core/src/lib.rs'], { rust: true, frontend: false, docs: false }, all],
    ['the shell crate alone', ['src-tauri/src/lib.rs'], { rust: true, frontend: false, docs: false }, ['velocity-desktop-v2']],
    ['Cargo.lock', ['Cargo.lock'], { rust: true, frontend: true, docs: true }, all],
    ['package-lock.json (not used by V2, so unknown)', ['package-lock.json'], { rust: true, frontend: true, docs: true }, all],
    ['pnpm-lock.yaml', ['pnpm-lock.yaml'], { rust: true, frontend: true, docs: true }, all],
    ['a workflow file', ['.github/workflows/pr.yml'], { rust: true, frontend: true, docs: true }, all],
    ['a native lock file inside a crate', ['crates/velocity-core/deps.lock'], { rust: true, frontend: true, docs: true }, all],
    ['a vcpkg manifest', ['src-tauri/vcpkg.json'], { rust: true, frontend: true, docs: true }, all],
    ['an unknown path', ['tools/new-thing.sh'], { rust: true, frontend: true, docs: true }, all],
  ];

  it.each(cases)('%s', (_name, paths, expected, packages) => {
    const s = selectJobs(paths, workspace);
    expect({ rust: s.jobs.rust.run, frontend: s.jobs.frontend.run, docs: s.jobs.docs.run }).toEqual(expected);
    expect(s.rustPackages).toEqual(packages);
  });

  it('gives every job a reason, including skipped ones', () => {
    const s = selectJobs(['docs/CI.md'], workspace);
    for (const job of Object.values(s.jobs)) expect(job.reason).toMatch(/\S/);
    expect(s.jobs.rust.reason).toMatch(/no Rust/i);
    expect(s.jobs.docs.reason).toContain('docs/CI.md');
  });

  it('selects nothing for an empty change, with reasons', () => {
    const s = selectJobs([], workspace);
    expect(Object.values(s.jobs).map((j) => j.run)).toEqual([false, false, false, false]);
    expect(s.jobs.rust.reason).toMatch(/no changed paths/i);
  });

  it('names the path that forced everything', () => {
    expect(selectJobs(['.github/workflows/pr.yml'], workspace).jobs.frontend.reason).toContain('.github/workflows/pr.yml');
  });

  // With the libcurl layer as a workspace crate, as in this repository since #35.
  const withCurl = { crates: [...workspace.crates, { name: 'velocity-curl', dir: 'crates/velocity-curl', deps: [] }] };

  // The native builds (#35) take minutes, and on Windows bill at twice the Linux rate, so they run
  // for their own inputs, CI changes and unknown paths, not for every shared file.
  it.each([
    ['the native build scripts', ['scripts/native/build.mjs'], true],
    ['the native lock file', ['scripts/native/deps.lock'], true],
    ['a native lock file inside a crate', ['crates/velocity-core/deps.lock'], true],
    ['the libcurl layer, which links the native build', ['crates/velocity-curl/build.rs'], true],
    ['the Cargo link overrides', ['.cargo/config.toml'], true],
    ['a workflow file', ['.github/workflows/pr.yml'], true],
    ['an unknown path', ['tools/new-thing.sh'], true],
    ['Cargo.lock', ['Cargo.lock'], false],
    ['pnpm-lock.yaml', ['pnpm-lock.yaml'], false],
    ['another check script', ['scripts/checks-lib.mjs'], false],
    ['a crate', ['crates/velocity-core/src/lib.rs'], false],
    ['docs only', ['docs/CI.md'], false],
  ])('runs the native builds for %s: %s', (_name, paths, run) => {
    expect(selectJobs(paths, withCurl).jobs.native.run).toBe(run);
  });

  it('says why the native builds run or are skipped', () => {
    expect(selectJobs(['scripts/native/deps.lock', 'docs/CI.md'], workspace).jobs.native.reason).toBe('native build inputs changed: scripts/native/deps.lock');
    expect(selectJobs(['crates/velocity-core/src/lib.rs'], workspace).jobs.native.reason).toBe('no native build input changed');
  });
});

describe('jobSummaryMarkdown (run summary, #13)', () => {
  it('lists every job as run or skipped, with its reason', () => {
    const md = jobSummaryMarkdown(selectJobs(['docs/CI.md'], workspace));
    expect(md).toMatch(/\| rust \| skipped \| no Rust crate changed \|/);
    expect(md).toMatch(/\| frontend \| skipped \| no frontend file changed \|/);
    expect(md).toMatch(/\| docs \| runs \| documentation changed: docs\/CI\.md \|/);
  });

  it('lists the changed paths and why each matters', () => {
    const md = jobSummaryMarkdown(selectJobs(['Cargo.lock', 'docs/CI.md'], workspace));
    expect(md).toContain('Cargo.lock: shared file, selects everything');
    expect(md).toContain('docs/CI.md: documentation, no code checks');
  });

  it('escapes pipes in paths so the table stays intact', () => {
    expect(jobSummaryMarkdown(selectJobs(['weird|name.txt'], workspace))).toContain('weird\\|name.txt');
  });
});
