#!/usr/bin/env node
// `pnpm check:prereqs`: checks the prerequisites for building Velocity Download Manager V2 and prints
// the documented fix for anything missing. Exits 1 if a prerequisite is missing. Changes nothing.
import { spawnSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { evaluate, parsePackageManager, parseToolchainChannel } from './doctor-lib.mjs';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');

/**
 * Runs a command and returns its first output line, or null. No shell, except for the constant
 * `pnpm --version` on Windows: Node refuses to start .cmd shims without one.
 */
function probe(cmd, args, { shell = false } = {}) {
  const r = spawnSync(cmd, args, { encoding: 'utf8', windowsHide: true, shell });
  if (r.error || r.status !== 0) return null;
  return `${r.stdout}${r.stderr}`.trim().split(/\r?\n/)[0] || null;
}

/** rustup/cargo may be installed but not yet on PATH in this shell (common right after install). */
function cargoTool(name) {
  return probe(name, ['--version']) ?? probe(join(homedir(), '.cargo', 'bin', name), ['--version']);
}

function windowsMsvc() {
  const vswhere = join(process.env['ProgramFiles(x86)'] ?? 'C:\\Program Files (x86)', 'Microsoft Visual Studio', 'Installer', 'vswhere.exe');
  if (!existsSync(vswhere)) return false;
  const out = probe(vswhere, ['-products', '*', '-requires', 'Microsoft.VisualStudio.Component.VC.Tools.x86.x64', '-property', 'installationPath']);
  return Boolean(out);
}

/** gpg on PATH, or the copy Git for Windows ships (scripts/native/fetch.mjs looks in the same places). */
function gpg() {
  return probe('gpg', ['--version']) ?? (platform === 'win32' ? probe('C:\\Program Files\\Git\\usr\\bin\\gpg.exe', ['--version']) : null);
}

/** cmake on PATH, or the copy Visual Studio installs (scripts/native/build.mjs uses either). */
function windowsCmake() {
  const onPath = probe('cmake', ['--version']);
  if (onPath) return onPath;
  const vswhere = join(process.env['ProgramFiles(x86)'] ?? 'C:\\Program Files (x86)', 'Microsoft Visual Studio', 'Installer', 'vswhere.exe');
  const vs =existsSync(vswhere) ? probe(vswhere, ['-latest', '-products', '*', '-property', 'installationPath']) : null;
  return vs ? probe(join(vs, 'Common7', 'IDE', 'CommonExtensions', 'Microsoft', 'CMake', 'CMake', 'bin', 'cmake.exe'), ['--version']) : null;
}

function windowsWebView2() {
  const key = '{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}';
  for (const hive of [`HKLM\\SOFTWARE\\WOW6432Node\\Microsoft\\EdgeUpdate\\Clients\\${key}`, `HKCU\\Software\\Microsoft\\EdgeUpdate\\Clients\\${key}`]) {
    const r = spawnSync('reg', ['query', hive, '/v', 'pv'], { encoding: 'utf8', windowsHide: true });
    const m = /pv\s+REG_SZ\s+(\S+)/.exec(r.stdout ?? '');
    if (m && m[1] !== '0.0.0.0') return m[1];
  }
  return null;
}

const platform = process.platform;
const pkg = JSON.parse(readFileSync(join(root, 'package.json'), 'utf8'));
const toolchain = parseToolchainChannel(readFileSync(join(root, 'rust-toolchain.toml'), 'utf8'));
const pinned = parsePackageManager(pkg.packageManager);
if (pinned?.name !== 'pnpm') {
  console.error('package.json must pin pnpm in its packageManager field.');
  process.exit(1);
}

const findings = evaluate(
  {
    platform,
    node: process.version,
    pnpm: probe('pnpm', ['--version'], { shell: platform === 'win32' }),
    rustup: cargoTool('rustup'),
    cargo: cargoTool('cargo'),
    msvc: platform === 'win32' ? windowsMsvc() : false,
    webview2: platform === 'win32' ? windowsWebView2() : null,
    webkitgtk: platform === 'linux' ? probe('pkg-config', ['--modversion', 'webkit2gtk-4.1']) : null,
    xcode: platform === 'darwin' ? probe('xcode-select', ['-p']) : null,
    gpg: platform === 'darwin' ? null : gpg(),
    cmake: platform === 'win32' ? windowsCmake() : null,
    make: platform === 'linux' ? probe('make', ['--version']) : null,
    // `perl --version` starts with a blank line, which `probe` would read as no output.
    perl: platform === 'linux' ? probe('perl', ['-e', 'print $^V']) : null,
  },
  { node: pkg.engines.node, pnpm: pinned.version },
);

console.log(`Velocity Download Manager V2 — prerequisite check (${platform}, pinned Rust toolchain ${toolchain ?? 'none'})\n`);
for (const f of findings) {
  console.log(`${f.ok ? 'OK     ' : 'MISSING'} ${f.name}: ${f.detail}`);
  if (!f.ok) console.log(`        Fix: ${f.fix}`);
}
const missing = findings.filter((f) => !f.ok).length;
console.log(missing ? `\n${missing} prerequisite(s) missing. See docs/DEVELOPMENT.md.` : '\nAll prerequisites found. Next: `pnpm install --frozen-lockfile`, then `pnpm dev`.');
process.exit(missing ? 1 : 0);
