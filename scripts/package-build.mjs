// The steps `pnpm package:windows` and `pnpm package:linux` share (docs/PACKAGING.md): build the
// sidecars for release as the release identity, stage them and the files the packages carry, run
// `tauri build` for the platform's packages, write the platform's records for a release candidate
// (#17), and write each package's SHA-256 beside it. Tools run directly, never through a shell.
// The rules are in package-lib.mjs.
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { copyFileSync, existsSync, mkdirSync, readdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { root, toolCommand } from './checks-runner.mjs';
import { hostTriple, SIDECARS, sidecarBuildArgs, sidecarSource, STAGED_FILES, tauriBuildArgs } from './package-lib.mjs';

/** Runs `command` with `args` from the repository root, and throws if it fails. */
export function run(command, args, options = {}) {
  console.log(`> ${[command, ...args].join(' ')}`);
  const r = spawnSync(command, args, { cwd: root, stdio: 'inherit', windowsHide: true, ...options });
  if (r.error) throw r.error;
  if (r.status !== 0) throw new Error(`${command} exited with ${r.status}`);
}

/**
 * Builds and stages everything, then runs `tauri build` for `platform` (`windows` or `linux`).
 * Returns the host's target triple.
 */
export function buildPackages(platform) {
  const rustc = spawnSync('rustc', ['-vV'], { cwd: root, encoding: 'utf8', windowsHide: true });
  if (rustc.status !== 0) throw new Error(`rustc -vV failed:\n${rustc.stderr}`);
  const triple = hostTriple(rustc.stdout);
  const ext = platform === 'windows' ? '.exe' : '';

  const [cargo] = toolCommand('cargo');
  run(cargo, sidecarBuildArgs());

  mkdirSync(join(root, 'src-tauri', 'binaries'), { recursive: true });
  for (const { program } of SIDECARS) {
    copyFileSync(join(root, 'target', 'release', `${program}${ext}`), join(root, sidecarSource(program, triple)));
  }
  const staging = join(root, 'src-tauri', 'staging');
  mkdirSync(staging, { recursive: true });
  const stagedFrom = { 'THIRD_PARTY_NOTICES.md': join(root, 'THIRD_PARTY_NOTICES.md'), 'build-info.json': join(root, 'target', 'native', triple, 'build-info.json') };
  for (const file of STAGED_FILES) {
    if (!existsSync(stagedFrom[file])) throw new Error(`${stagedFrom[file]} is missing${file === 'build-info.json' ? ': run `pnpm native:build` first' : ''}`);
    copyFileSync(stagedFrom[file], join(staging, file));
  }

  run(process.execPath, [join(root, 'node_modules', '@tauri-apps', 'cli', 'tauri.js'), ...tauriBuildArgs(platform)]);

  // The records a release candidate carries for this platform (#17): the native libraries' and
  // libtorrent's build records, and the licence report of what this platform's build ships.
  const bundle = join(root, 'target', 'release', 'bundle');
  for (const record of ['build-info.json', 'libtorrent-info.json']) {
    const from = join(root, 'target', 'native', triple, record);
    if (!existsSync(from)) throw new Error(`${from} is missing: run \`pnpm native:build\` first`);
    copyFileSync(from, join(bundle, `${record === 'build-info.json' ? 'native-build-info' : 'libtorrent-info'}-${platform}.json`));
  }
  run(process.execPath, [join(root, 'scripts', 'notices.mjs'), '--report', triple, join(bundle, `licence-report-${platform}.json`)]);
  return triple;
}

/**
 * The one package in `target/release/bundle/<kind>/` whose name ends with `suffix`, with its SHA-256
 * written to `<package>.sha256` in the `sha256sum` format. Returns its path.
 */
export function finishPackage(kind, suffix) {
  const dir = join(root, 'target', 'release', 'bundle', kind);
  const found = readdirSync(dir).filter((f) => f.endsWith(suffix));
  if (found.length !== 1) throw new Error(`expected one ${suffix} in ${dir}, found: ${found.join(', ') || 'none'}`);
  const file = join(dir, found[0]);
  const sha256 = createHash('sha256').update(readFileSync(file)).digest('hex');
  writeFileSync(`${file}.sha256`, `${sha256}  ${found[0]}\n`);
  console.log(`Package: ${file}\nSHA-256: ${sha256}`);
  return file;
}
