// Pure logic for `pnpm check:prereqs` (scripts/doctor.mjs): version parsing, range checks and the
// per-platform list of findings with their documented fixes. Probing the machine happens in
// doctor.mjs so this module stays testable.
//
// Fixes follow the official Tauri prerequisites: https://v2.tauri.app/start/prerequisites/

/** First `major.minor.patch` in a tool's output, or null. */
export function parseVersion(text) {
  const m = /(\d+)\.(\d+)\.(\d+)/.exec(text ?? '');
  return m ? [Number(m[1]), Number(m[2]), Number(m[3])] : null;
}

function compare(a, b) {
  for (let i = 0; i < 3; i++) {
    if (a[i] !== b[i]) return a[i] < b[i] ? -1 : 1;
  }
  return 0;
}

/** Supports the subset of semver ranges used in package.json engines: `^x.y.z`, `>=x.y.z`, joined by `||`. */
export function satisfies(version, range) {
  return range.split('||').some((part) => {
    const alt = part.trim();
    const min = parseVersion(alt);
    if (!min) return false;
    if (alt.startsWith('>=')) return compare(version, min) >= 0;
    if (alt.startsWith('^')) return version[0] === min[0] && compare(version, min) >= 0;
    return compare(version, min) === 0;
  });
}

/** The `channel` pinned in rust-toolchain.toml, or null. */
export function parseToolchainChannel(toml) {
  const m = /^\s*channel\s*=\s*"([^"]+)"/m.exec(toml ?? '');
  return m ? m[1] : null;
}

/** `{ name, version }` from package.json's `packageManager` (e.g. `pnpm@12.14.0+sha512...`), or null. */
export function parsePackageManager(field) {
  const m = /^([a-z]+)@(\d+\.\d+\.\d+)/.exec(field ?? '');
  return m ? { name: m[1], version: m[2] } : null;
}

const RUST_FIX = {
  win32: 'Install rustup: `winget install --id Rustlang.Rustup`, choose the MSVC toolchain (x86_64-pc-windows-msvc), then open a new terminal.',
  default: "Install rustup: `curl --proto '=https' --tlsv1.2 https://sh.rustup.rs -sSf | sh`, then open a new terminal.",
};

/**
 * Turns probe results into findings: { name, ok, detail, fix }.
 * @param {{platform: string, node: string|null, pnpm: string|null, rustup: string|null, cargo: string|null,
 *          msvc: boolean, webview2: string|null, webkitgtk: string|null, xcode: string|null}} probe
 * @param {{node: string, pnpm: string}} requirements
 */
export function evaluate(probe, requirements) {
  const findings = [];
  const nodeVersion = parseVersion(probe.node);
  findings.push({
    name: 'Node.js',
    ok: nodeVersion !== null && satisfies(nodeVersion, requirements.node),
    detail: nodeVersion ? `found ${nodeVersion.join('.')}, need ${requirements.node}` : 'not found',
    fix: `Install a Node.js LTS release matching ${requirements.node} from https://nodejs.org/ (or your version manager).`,
  });

  const pnpmVersion = parseVersion(probe.pnpm);
  findings.push({
    name: 'pnpm',
    ok: pnpmVersion !== null && pnpmVersion.join('.') === requirements.pnpm,
    detail: pnpmVersion ? `found ${pnpmVersion.join('.')}, project pins ${requirements.pnpm}` : 'not found',
    fix: `Run \`corepack enable\` (Corepack then uses the version pinned in package.json), or install pnpm ${requirements.pnpm} with \`npx get-pnpm\` (https://pnpm.io/installation).`,
  });

  const rustOk = Boolean(probe.rustup && parseVersion(probe.cargo));
  findings.push({
    name: 'Rust (rustup and cargo)',
    ok: rustOk,
    detail: rustOk ? `${probe.cargo}; rust-toolchain.toml selects the pinned toolchain automatically` : 'rustup or cargo not found on PATH',
    fix: RUST_FIX[probe.platform] ?? RUST_FIX.default,
  });

  if (probe.platform === 'win32') {
    findings.push({
      name: 'Microsoft C++ Build Tools',
      ok: probe.msvc,
      detail: probe.msvc ? 'MSVC x64 tools found' : 'no Visual Studio installation with the MSVC x64 tools',
      fix: 'Install Microsoft C++ Build Tools with the "Desktop development with C++" workload: https://visualstudio.microsoft.com/visual-cpp-build-tools/',
    });
    findings.push({
      name: 'WebView2 Runtime',
      ok: Boolean(probe.webview2),
      detail: probe.webview2 ? `version ${probe.webview2}` : 'not installed',
      fix: 'Install the WebView2 Evergreen Bootstrapper: https://developer.microsoft.com/microsoft-edge/webview2/',
    });
  } else if (probe.platform === 'linux') {
    findings.push({
      name: 'WebKitGTK 4.1 development files',
      ok: Boolean(probe.webkitgtk),
      detail: probe.webkitgtk ? `webkit2gtk-4.1 ${probe.webkitgtk}` : 'pkg-config cannot find webkit2gtk-4.1',
      fix: 'Debian/Ubuntu: `sudo apt update && sudo apt install libwebkit2gtk-4.1-dev build-essential curl wget file libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev` (other distributions: https://v2.tauri.app/start/prerequisites/#linux).',
    });
  } else if (probe.platform === 'darwin') {
    findings.push({
      name: 'Xcode Command Line Tools',
      ok: Boolean(probe.xcode),
      detail: probe.xcode ? `installed at ${probe.xcode}` : 'not installed',
      fix: 'Run `xcode-select --install` (or install Xcode and launch it once).',
    });
  }
  return findings;
}
