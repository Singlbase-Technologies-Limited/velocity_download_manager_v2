// Pure logic behind `pnpm package:windows` (#81, docs/PACKAGING.md): which programs and files the
// installer carries, the commands that build it, and the check of what ended up inside it. No I/O
// here, so every rule is unit tested (package-lib.test.mjs).

/**
 * The programs the app starts from beside its own executable, as `cargo build` names them, with
 * the package that builds each. Their names are the ones the app looks for
 * (`src-tauri/src/downloads.rs`, `src-tauri/src/browser.rs`); a test in src-tauri holds
 * tauri.release.conf.json's `externalBin` to the same list.
 */
export const SIDECARS = [
  { program: 'velocity-transfer', package: 'velocity-transfer' },
  { program: 'velocity-torrent', package: 'velocity-torrent' },
  { program: 'velocity-native-host-v2', package: 'velocity-native-host' },
];

/** Files installed beside the programs, from `src-tauri/staging/` (tauri.release.conf.json). */
export const STAGED_FILES = ['THIRD_PARTY_NOTICES.md', 'build-info.json'];

/** The Cargo feature that makes a build run as the release identity (branding.json, #81). */
export const RELEASE_FEATURE = 'velocity-core/release-identity';

/**
 * Where Tauri wants a sidecar before bundling: `src-tauri/binaries/<program>-<target triple><ext>`.
 * It installs it beside the app as `<program><ext>`.
 */
export function sidecarSource(program, triple) {
  const ext = triple.includes('windows') ? '.exe' : '';
  return `src-tauri/binaries/${program}-${triple}${ext}`;
}

/** The host's target triple, from `rustc -vV`. */
export function hostTriple(rustcVersion) {
  const m = /^host: (\S+)$/m.exec(rustcVersion);
  if (!m) throw new Error('rustc -vV printed no host triple');
  return m[1];
}

/** The release build of every sidecar, with the release identity, as one cargo command's arguments. */
export function sidecarBuildArgs() {
  return ['build', '--release', '--locked', ...SIDECARS.flatMap((s) => ['-p', s.package]), '--features', RELEASE_FEATURE];
}

/** The Tauri CLI's arguments for the Windows installer. */
export function tauriBuildArgs() {
  return ['build', '--config', 'src-tauri/tauri.release.conf.json', '--features', 'release-identity', '--bundles', 'nsis'];
}

/**
 * Checks a listing of the installer's files (`7z l -slt`: one `Path = …` line per entry) and
 * returns the problems: an aria2 file in it (ADR-0001), or a program or file it must carry and
 * doesn't. An empty list means the installer is complete.
 */
export function installerProblems(listing) {
  const names = listing
    .split(/\r?\n/)
    .map((l) => /^Path = (.+)$/.exec(l)?.[1])
    .filter(Boolean)
    .map((p) => p.split(/[\\/]/).pop().toLowerCase());
  const problems = [];
  for (const n of names.filter((n) => n.includes('aria2'))) problems.push(`it contains ${n}: V2 never ships aria2 (ADR-0001)`);
  for (const want of [...SIDECARS.map((s) => `${s.program}.exe`), ...STAGED_FILES]) {
    if (!names.includes(want.toLowerCase())) problems.push(`it lacks ${want}`);
  }
  return problems;
}
