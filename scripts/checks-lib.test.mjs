import { describe, expect, it } from 'vitest';
import { checkCommands, focusedCommands, selectChecks, workspaceFromMetadata } from './checks-lib.mjs';

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
    expect(cmds.map((c) => c.id)).toEqual(['rust-fmt', 'rust-clippy', 'rust-test']);
    expect(cmds.find((c) => c.id === 'rust-test')?.args).toEqual(['test', '-p', 'velocity-core', '--no-fail-fast']);
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
      { id: 'rust-test', tool: 'cargo', args: ['test', '-p', 'velocity-core', 'range_probe'], name: 'cargo test -p velocity-core' },
    ]);
  });

  it('accepts a leading -- from the package manager', () => {
    expect(focusedCommands(['--', 'velocity-core'], workspace)[0]?.args).toEqual(['test', '-p', 'velocity-core']);
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
