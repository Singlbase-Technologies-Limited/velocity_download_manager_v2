// Pure logic behind `pnpm package:windows` (#81) and `pnpm package:linux` (#82)
// (docs/PACKAGING.md): which programs and files the packages carry, the commands that build them,
// and the checks of what ended up inside them. No I/O here, so every rule is unit tested
// (package-lib.test.mjs).

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

/** The app's own program: src-tauri's Cargo package name. */
export const APP_PROGRAM = 'velocity-desktop-v2';

/** The release product name (branding.json's `releaseProductName`, ADR-0016). */
export const PRODUCT_NAME = 'Velocity Download Manager V2';

/** What Tauri builds on each platform (PO decisions on #81 and #82: no MSI, no AppImage). */
export const BUNDLES = { windows: ['nsis'], linux: ['deb', 'rpm'] };

/**
 * The newest glibc the Linux packages may need: Ubuntu 22.04's, where they are built (PO decision
 * on #82), so they run there and on anything newer.
 */
export const GLIBC_BASELINE = '2.35';

/**
 * Libraries V2 builds and links statically (docs/BUILDING_NATIVE.md). A program in a package that
 * loads a system copy of one instead would bring that distribution's build and licence terms,
 * which docs/LICENSING.md rules out without review.
 */
export const STATIC_ONLY = ['libcurl', 'libssl', 'libcrypto', 'libnghttp2', 'libz.', 'libtorrent', 'libboost', 'libssh', 'libidn2', 'librtmp', 'libaria2'];

/**
 * Where V1's Linux packages put files, from V1's tauri.conf.json at e9d717a8 (product name
 * "Velocity Download Manager", main binary `velocity-desktop`, sidecars `velocity-aria2c` and
 * `velocity-native-host`) and Tauri's layout. V2's packages must not touch any of them.
 */
export const V1_LINUX_PATHS = {
  exact: [
    'usr/bin/velocity-desktop',
    'usr/bin/velocity-aria2c',
    'usr/bin/velocity-native-host',
    'usr/share/applications/Velocity Download Manager.desktop',
  ],
  under: ['usr/lib/Velocity Download Manager/'],
  /** V1's icons: `usr/share/icons/hicolor/<size>/apps/velocity-desktop.png`. */
  icon: /^usr\/share\/icons\/hicolor\/[^/]+\/apps\/velocity-desktop\.png$/,
};

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

/** The Tauri CLI's arguments for `platform`'s packages (`windows` or `linux`). */
export function tauriBuildArgs(platform) {
  const bundles = BUNDLES[platform];
  if (!bundles) throw new Error(`no packages for ${platform}`);
  return ['build', '--config', 'src-tauri/tauri.release.conf.json', '--features', 'release-identity', '--bundles', bundles.join(',')];
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

/** The files a Linux package must install, as paths without a leading `/`. */
export function linuxPackageFiles() {
  return [
    ...[APP_PROGRAM, ...SIDECARS.map((s) => s.program)].map((p) => `usr/bin/${p}`),
    ...STAGED_FILES.map((f) => `usr/lib/${PRODUCT_NAME}/${f}`),
    `usr/share/applications/${PRODUCT_NAME}.desktop`,
  ];
}

/**
 * The paths a listing of a deb (`dpkg-deb -c`: permissions, owner, size, date, time, then the path,
 * with ` -> target` for a link) or an rpm (`rpm -qlp`: one absolute path a line) installs, without
 * a leading `./` or `/`, and without directories. Tauri's debs list `usr/bin/…`, with no `./`, and
 * directories with no trailing `/`, so a deb's directories are known by their `d` permission.
 */
export function packagePaths(listing) {
  return listing
    .split(/\r?\n/)
    .map((l) => l.trim())
    .filter(Boolean)
    .map((l) => {
      const deb = /^([-dlbcps])[-rwxsStT]{9}\s+\S+\s+\d+\s+\S+\s+\S+\s+(?:\.\/)?(.*?)(?: -> .*)?$/.exec(l);
      if (!deb) return l.replace(/^\/+/, '');
      return deb[1] === 'd' ? '' : deb[2];
    })
    .filter((p) => p && !p.endsWith('/'));
}

/**
 * Checks the paths a Linux package installs (from `packagePaths`) and returns the problems: aria2
 * (ADR-0001), a file it must carry and doesn't, or a path V1's packages own, which would stop one
 * of the two from installing beside the other.
 */
export function linuxPackageProblems(paths) {
  const problems = [];
  for (const p of paths.filter((p) => p.toLowerCase().includes('aria2'))) problems.push(`it contains ${p}: V2 never ships aria2 (ADR-0001)`);
  for (const p of paths) {
    const v1 = V1_LINUX_PATHS.exact.includes(p) || V1_LINUX_PATHS.under.some((d) => p.startsWith(d)) || V1_LINUX_PATHS.icon.test(p);
    if (v1) problems.push(`it installs ${p}, which V1's package owns`);
  }
  for (const want of linuxPackageFiles()) {
    if (!paths.includes(want)) problems.push(`it lacks ${want}`);
  }
  return problems;
}

/** The libraries an ELF file loads, from `readelf -d` (`(NEEDED) Shared library: [libc.so.6]`). */
export function neededLibraries(readelf) {
  return [...readelf.matchAll(/\(NEEDED\)\s+Shared library: \[([^\]]+)\]/g)].map((m) => m[1]);
}

/** The problems with what `file` loads: any system copy of a library V2 links statically. */
export function neededProblems(file, libraries) {
  return libraries.filter((l) => STATIC_ONLY.some((s) => l.startsWith(s))).map((l) => `${file} loads the system's ${l}, which V2 links statically`);
}

/** Compares two dotted versions: negative, zero or positive. */
function compareVersions(a, b) {
  const [x, y] = [a, b].map((v) => v.split('.').map(Number));
  for (let i = 0; i < Math.max(x.length, y.length); i++) {
    const d = (x[i] ?? 0) - (y[i] ?? 0);
    if (d) return d;
  }
  return 0;
}

/**
 * The newest glibc symbol version an ELF file needs, from `objdump -T` (`… GLIBC_2.34 …`), or null
 * when it needs none.
 */
export function newestGlibc(objdump) {
  const versions = [...objdump.matchAll(/\bGLIBC_(\d+(?:\.\d+)+)\b/g)].map((m) => m[1]);
  return versions.reduce((best, v) => (best === null || compareVersions(v, best) > 0 ? v : best), null);
}

/** The problem when `file` needs a glibc newer than the baseline, or null. */
export function glibcProblem(file, newest, baseline = GLIBC_BASELINE) {
  if (newest === null || compareVersions(newest, baseline) <= 0) return null;
  return `${file} needs glibc ${newest}, newer than the ${baseline} the packages support (Ubuntu 22.04)`;
}
