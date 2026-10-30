import { describe, expect, it } from 'vitest';
import { hostTriple, installerProblems, SIDECARS, sidecarBuildArgs, sidecarSource, STAGED_FILES, tauriBuildArgs } from './package-lib.mjs';

describe('sidecars', () => {
  it('are staged under the names Tauri expects, with the target triple', () => {
    expect(sidecarSource('velocity-transfer', 'x86_64-pc-windows-msvc')).toBe('src-tauri/binaries/velocity-transfer-x86_64-pc-windows-msvc.exe');
    expect(sidecarSource('velocity-transfer', 'x86_64-unknown-linux-gnu')).toBe('src-tauri/binaries/velocity-transfer-x86_64-unknown-linux-gnu');
  });

  it('are the engine, the torrent helper and the native host', () => {
    expect(SIDECARS.map((s) => s.program)).toEqual(['velocity-transfer', 'velocity-torrent', 'velocity-native-host-v2']);
  });

  it('are built for release, locked, as the release identity', () => {
    expect(sidecarBuildArgs()).toEqual([
      'build',
      '--release',
      '--locked',
      '-p',
      'velocity-transfer',
      '-p',
      'velocity-torrent',
      '-p',
      'velocity-native-host',
      '--features',
      'velocity-core/release-identity',
    ]);
  });
});

describe('the Tauri build', () => {
  it('uses the release configuration and identity, and makes only the NSIS installer', () => {
    expect(tauriBuildArgs()).toEqual(['build', '--config', 'src-tauri/tauri.release.conf.json', '--features', 'release-identity', '--bundles', 'nsis']);
  });
});

describe('hostTriple', () => {
  it("reads rustc's host line", () => {
    expect(hostTriple('rustc 1.90.0\nbinary: rustc\nhost: x86_64-pc-windows-msvc\nrelease: 1.90.0\n')).toBe('x86_64-pc-windows-msvc');
  });

  it('refuses output without one', () => {
    expect(() => hostTriple('rustc 1.90.0\n')).toThrow(/no host triple/);
  });
});

describe('installerProblems', () => {
  const entry = (p) => `Path = ${p}\r\nSize = 1\r\n`;
  const complete = ['velocity-desktop-v2.exe', 'velocity-transfer.exe', 'velocity-torrent.exe', 'velocity-native-host-v2.exe', ...STAGED_FILES, 'uninstall.exe']
    .map(entry)
    .join('\r\n');

  it('passes an installer with every program and file', () => {
    expect(installerProblems(complete)).toEqual([]);
  });

  it('refuses aria2 anywhere in it', () => {
    expect(installerProblems(complete + entry('$INSTDIR\\bin\\aria2c.exe'))).toEqual(['it contains aria2c.exe: V2 never ships aria2 (ADR-0001)']);
  });

  it('names what is missing', () => {
    const without = complete.replace(entry('velocity-torrent.exe'), '').replace(entry('build-info.json'), '');
    expect(installerProblems(without)).toEqual(['it lacks velocity-torrent.exe', 'it lacks build-info.json']);
  });
});
