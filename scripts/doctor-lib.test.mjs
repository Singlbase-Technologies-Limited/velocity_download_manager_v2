import { describe, expect, it } from 'vitest';
import { evaluate, parsePackageManager, parseToolchainChannel, parseVersion, satisfies } from './doctor-lib.mjs';

describe('parseVersion', () => {
  it('reads versions from tool output', () => {
    expect(parseVersion('v24.12.0')).toEqual([24, 12, 0]);
    expect(parseVersion('rustc 1.99.1 (58cf8f9c3 2026-10-13)')).toEqual([1, 99, 1]);
    expect(parseVersion('cargo 1.99.1 (abc 2026-10-01)')).toEqual([1, 99, 1]);
  });
  it('returns null when there is no version', () => {
    expect(parseVersion('command not found')).toBeNull();
    expect(parseVersion('')).toBeNull();
  });
});

describe('satisfies', () => {
  const range = '^22.13.0 || ^24.0.0 || >=26.0.0';
  it('accepts versions inside any alternative', () => {
    expect(satisfies([22, 13, 0], range)).toBe(true);
    expect(satisfies([24, 12, 0], range)).toBe(true);
    expect(satisfies([26, 1, 0], range)).toBe(true);
  });
  it('rejects versions outside every alternative', () => {
    expect(satisfies([22, 12, 9], range)).toBe(false);
    expect(satisfies([23, 0, 0], range)).toBe(false);
    expect(satisfies([20, 19, 0], range)).toBe(false);
  });
});

describe('parseToolchainChannel', () => {
  it('reads the pinned channel from rust-toolchain.toml', () => {
    expect(parseToolchainChannel('[toolchain]\nchannel = "1.99.1"\ncomponents = ["rustfmt"]\n')).toBe('1.99.1');
  });
  it('returns null when no channel is pinned', () => {
    expect(parseToolchainChannel('[toolchain]\n')).toBeNull();
  });
});

describe('parsePackageManager', () => {
  it('reads the pinned pnpm version from package.json', () => {
    expect(parsePackageManager('pnpm@12.14.0+sha512.abc')).toEqual({ name: 'pnpm', version: '12.14.0' });
    expect(parsePackageManager('pnpm@12.14.0')).toEqual({ name: 'pnpm', version: '12.14.0' });
  });
  it('returns null for a missing or malformed field', () => {
    expect(parsePackageManager(undefined)).toBeNull();
    expect(parsePackageManager('pnpm')).toBeNull();
  });
});

const healthy = {
  platform: 'win32',
  node: 'v24.12.0',
  pnpm: '12.14.0',
  rustup: 'rustup 1.29.0',
  cargo: 'cargo 1.99.1 (abc 2026-10-01)',
  msvc: true,
  webview2: '155.0.4283.62',
  webkitgtk: null,
  xcode: null,
};
const requirements = { node: '^22.13.0 || ^24.0.0 || >=26.0.0', pnpm: '12.14.0' };
const failed = (findings) => findings.filter((f) => !f.ok).map((f) => f.name);

describe('evaluate', () => {
  it('passes a complete Windows setup', () => {
    expect(failed(evaluate(healthy, requirements))).toEqual([]);
  });

  it('reports a missing Rust toolchain with the install command', () => {
    const findings = evaluate({ ...healthy, rustup: null, cargo: null }, requirements);
    expect(failed(findings)).toEqual(['Rust (rustup and cargo)']);
    expect(findings.find((f) => !f.ok).fix).toContain('winget install --id Rustlang.Rustup');
  });

  it('reports an unsupported Node.js version', () => {
    const findings = evaluate({ ...healthy, node: 'v23.1.0' }, requirements);
    expect(failed(findings)).toEqual(['Node.js']);
    expect(findings.find((f) => !f.ok).detail).toContain('23.1.0');
  });

  it('reports a missing pnpm with the Corepack and npm installation routes', () => {
    const findings = evaluate({ ...healthy, pnpm: null }, requirements);
    expect(failed(findings)).toEqual(['pnpm']);
    const fix = findings.find((f) => !f.ok).fix;
    expect(fix).toContain('corepack enable');
    expect(fix).toContain('npx get-pnpm');
  });

  it('reports a pnpm version different from the pinned one', () => {
    const findings = evaluate({ ...healthy, pnpm: '10.33.1' }, requirements);
    expect(failed(findings)).toEqual(['pnpm']);
    expect(findings.find((f) => !f.ok).detail).toContain('10.33.1');
  });

  it('reports missing MSVC build tools and WebView2 on Windows', () => {
    expect(failed(evaluate({ ...healthy, msvc: false, webview2: null }, requirements))).toEqual([
      'Microsoft C++ Build Tools',
      'WebView2 Runtime',
    ]);
  });

  it('checks WebKitGTK on Linux with the apt packages from the Tauri docs', () => {
    const findings = evaluate({ ...healthy, platform: 'linux', msvc: false, webview2: null, webkitgtk: null }, requirements);
    expect(failed(findings)).toEqual(['WebKitGTK 4.1 development files']);
    expect(findings.find((f) => !f.ok).fix).toContain('libwebkit2gtk-4.1-dev');
    expect(failed(evaluate({ ...healthy, platform: 'linux', webkitgtk: '2.48.0' }, requirements))).toEqual([]);
  });

  it('checks the Xcode Command Line Tools on macOS', () => {
    const findings = evaluate({ ...healthy, platform: 'darwin', xcode: null }, requirements);
    expect(failed(findings)).toEqual(['Xcode Command Line Tools']);
    expect(findings.find((f) => !f.ok).fix).toContain('xcode-select --install');
  });
});
