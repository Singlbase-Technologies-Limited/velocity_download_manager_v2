// V2's version (#84, docs/RELEASE_PROCESS.md): one value, `package.json`'s, which everything else
// reads or must equal. No I/O here, so every rule is unit tested (version-lib.test.mjs).
//
// - package.json `version`: the source. The extension's manifest is built from it
//   (scripts/build-extension.mjs), and the release-candidate workflow checks tags against it.
// - src-tauri/tauri.conf.json `version`: "../package.json", so Tauri reads the same value for the
//   app and its packages (https://v2.tauri.app/reference/config/#version).
// - Cargo.toml `[workspace.package] version`: Cargo can't read another file, so it repeats the value,
//   and every crate takes it (`version.workspace = true`). The programs report it through
//   `CARGO_PKG_VERSION`.

/** A release version: `MAJOR.MINOR.PATCH`, without leading zeros (https://semver.org/). */
const SEMVER = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/;

export function isVersion(text) {
  return SEMVER.test(text);
}

/** `[workspace.package]`'s `version` in Cargo.toml's text, or null. */
export function cargoWorkspaceVersion(cargoToml) {
  const section = /^\[workspace\.package\]\s*$([\s\S]*?)(?=^\[|(?![\s\S]))/m.exec(cargoToml);
  if (!section) return null;
  return /^version\s*=\s*"([^"]*)"\s*$/m.exec(section[1])?.[1] ?? null;
}

/**
 * Where the versions disagree, as problems; an empty list when they agree. Each argument is the
 * file's text.
 */
export function versionProblems({ packageJson, cargoToml, tauriConf }) {
  const problems = [];
  const version = JSON.parse(packageJson).version;
  if (!isVersion(version ?? '')) problems.push(`package.json's version "${version}" isn't MAJOR.MINOR.PATCH`);
  const cargo = cargoWorkspaceVersion(cargoToml);
  if (cargo !== version) problems.push(`Cargo.toml's workspace version is "${cargo}", but package.json's is "${version}": run \`pnpm version:set ${version}\``);
  const tauri = JSON.parse(tauriConf).version;
  if (tauri !== '../package.json') problems.push(`src-tauri/tauri.conf.json's version is "${tauri}"; it must be "../package.json", so Tauri reads package.json's`);
  return problems;
}

/** package.json's and Cargo.toml's text with `version` set; tauri.conf.json reads package.json's. */
export function setVersion({ packageJson, cargoToml }, version) {
  if (!isVersion(version)) throw new Error(`"${version}" isn't MAJOR.MINOR.PATCH`);
  const pkg = packageJson.replace(/^(\s*"version":\s*")[^"]*(")/m, `$1${version}$2`);
  const cargo = cargoToml.replace(/^(\[workspace\.package\]\s*$[\s\S]*?^version\s*=\s*")[^"]*(")/m, `$1${version}$2`);
  if (JSON.parse(pkg).version !== version) throw new Error('package.json has no top-level "version" to set');
  if (cargoWorkspaceVersion(cargo) !== version) throw new Error('Cargo.toml has no [workspace.package] version to set');
  return { packageJson: pkg, cargoToml: cargo };
}
