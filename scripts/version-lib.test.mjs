import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';
import { cargoWorkspaceVersion, isVersion, setVersion, versionProblems } from './version-lib.mjs';

const root = join(import.meta.dirname, '..');
const read = (path) => readFileSync(join(root, path), 'utf8');

const packageJson = '{\n  "name": "velocity",\n  "private": true,\n  "version": "0.1.0",\n  "type": "module"\n}\n';
const cargoToml = '[workspace]\nmembers = ["a"]\n\n[workspace.package]\nversion = "0.1.0"\nedition = "2021"\n\n[workspace.dependencies]\nserde = { version = "1.0.0" }\n';
const tauriConf = '{ "productName": "x", "version": "../package.json" }';

describe("the repository's versions", () => {
  it('agree: package.json, Cargo.toml and tauri.conf.json (#84)', () => {
    expect(versionProblems({ packageJson: read('package.json'), cargoToml: read('Cargo.toml'), tauriConf: read('src-tauri/tauri.conf.json') })).toEqual([]);
  });
});

describe('versionProblems', () => {
  it('passes versions that agree', () => {
    expect(versionProblems({ packageJson, cargoToml, tauriConf })).toEqual([]);
  });

  it("names Cargo.toml when it differs from package.json, and how to fix it", () => {
    expect(versionProblems({ packageJson, cargoToml: cargoToml.replace('version = "0.1.0"', 'version = "0.2.0"'), tauriConf })).toEqual([
      'Cargo.toml\'s workspace version is "0.2.0", but package.json\'s is "0.1.0": run `pnpm version:set 0.1.0`',
    ]);
  });

  it('refuses a version of its own in tauri.conf.json', () => {
    expect(versionProblems({ packageJson, cargoToml, tauriConf: '{ "version": "0.1.0" }' })).toEqual([
      'src-tauri/tauri.conf.json\'s version is "0.1.0"; it must be "../package.json", so Tauri reads package.json\'s',
    ]);
  });

  it('refuses a package.json version that is not MAJOR.MINOR.PATCH', () => {
    const odd = packageJson.replace('0.1.0', '0.1.0-rc.1');
    expect(versionProblems({ packageJson: odd, cargoToml: cargoToml.replace('"0.1.0"', '"0.1.0-rc.1"'), tauriConf })).toEqual([
      'package.json\'s version "0.1.0-rc.1" isn\'t MAJOR.MINOR.PATCH',
    ]);
  });
});

describe('versions', () => {
  it('are MAJOR.MINOR.PATCH without leading zeros; release candidates are tags, not versions', () => {
    for (const v of ['0.1.0', '1.0.0', '10.20.30']) expect(isVersion(v), v).toBe(true);
    for (const v of ['0.1', '01.0.0', '0.1.0-rc.1', 'v0.1.0', '0.1.0 ']) expect(isVersion(v), v).toBe(false);
  });

  it("are read from Cargo.toml's [workspace.package], not a dependency's", () => {
    expect(cargoWorkspaceVersion(cargoToml)).toBe('0.1.0');
    expect(cargoWorkspaceVersion('[workspace.dependencies]\nserde = { version = "1.0.0" }\n')).toBeNull();
  });
});

describe('setVersion', () => {
  it('sets package.json and Cargo.toml, and nothing else', () => {
    const next = setVersion({ packageJson, cargoToml }, '0.2.0');
    expect(next.packageJson).toBe(packageJson.replace('"version": "0.1.0"', '"version": "0.2.0"'));
    expect(next.cargoToml).toBe(cargoToml.replace('[workspace.package]\nversion = "0.1.0"', '[workspace.package]\nversion = "0.2.0"'));
    expect(next.cargoToml).toContain('serde = { version = "1.0.0" }');
    expect(versionProblems({ ...next, tauriConf })).toEqual([]);
  });

  it('refuses what is not a version', () => {
    expect(() => setVersion({ packageJson, cargoToml }, 'v0.2.0')).toThrow('"v0.2.0" isn\'t MAJOR.MINOR.PATCH');
  });
});
