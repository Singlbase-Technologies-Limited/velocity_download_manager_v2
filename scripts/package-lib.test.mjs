import { describe, expect, it } from 'vitest';
import {
  glibcProblem,
  hostTriple,
  installerProblems,
  linuxPackageFiles,
  linuxPackageProblems,
  neededLibraries,
  neededProblems,
  newestGlibc,
  packagePaths,
  SIDECARS,
  sidecarBuildArgs,
  sidecarSource,
  STAGED_FILES,
  tauriBuildArgs,
} from './package-lib.mjs';

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
  it('uses the release configuration and identity, and makes only the NSIS installer on Windows', () => {
    expect(tauriBuildArgs('windows')).toEqual(['build', '--config', 'src-tauri/tauri.release.conf.json', '--features', 'release-identity', '--bundles', 'nsis']);
  });

  it('makes a deb and an rpm on Linux, and no AppImage (PO decision on #82)', () => {
    expect(tauriBuildArgs('linux')).toEqual(['build', '--config', 'src-tauri/tauri.release.conf.json', '--features', 'release-identity', '--bundles', 'deb,rpm']);
  });

  it('refuses a platform it has no packages for', () => {
    expect(() => tauriBuildArgs('macos')).toThrow(/no packages for macos/);
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

describe('Linux packages', () => {
  const files = linuxPackageFiles();
  const icon = 'usr/share/icons/hicolor/32x32/apps/velocity-desktop-v2.png';

  it('carry the app, its three programs, the notices, build-info.json and the desktop entry', () => {
    expect(files).toEqual([
      'usr/bin/velocity-desktop-v2',
      'usr/bin/velocity-transfer',
      'usr/bin/velocity-torrent',
      'usr/bin/velocity-native-host-v2',
      'usr/lib/Velocity Download Manager V2/THIRD_PARTY_NOTICES.md',
      'usr/lib/Velocity Download Manager V2/build-info.json',
      'usr/share/applications/Velocity Download Manager V2.desktop',
    ]);
  });

  it('reads the paths of a deb listing, without directories, and follows no link', () => {
    const listing = [
      'drwxr-xr-x root/root         0 2026-10-31 10:00 ./',
      'drwxr-xr-x root/root         0 2026-10-31 10:00 ./usr/bin/',
      '-rwxr-xr-x root/root  12345678 2026-10-31 10:00 ./usr/bin/velocity-desktop-v2',
      '-rw-r--r-- root/root      2048 2026-10-31 10:00 ./usr/share/applications/Velocity Download Manager V2.desktop',
      'lrwxrwxrwx root/root         0 2026-10-31 10:00 ./usr/bin/vdm -> velocity-desktop-v2',
    ].join('\n');
    expect(packagePaths(listing)).toEqual([
      'usr/bin/velocity-desktop-v2',
      'usr/share/applications/Velocity Download Manager V2.desktop',
      'usr/bin/vdm',
    ]);
  });

  it('reads the paths of an rpm listing', () => {
    expect(packagePaths('/usr/bin/velocity-desktop-v2\n/usr/lib/Velocity Download Manager V2/build-info.json\n')).toEqual([
      'usr/bin/velocity-desktop-v2',
      'usr/lib/Velocity Download Manager V2/build-info.json',
    ]);
  });

  it('pass a package with every file and nothing of V1', () => {
    expect(linuxPackageProblems([...files, icon])).toEqual([]);
  });

  it("refuse aria2 and any path V1's package owns, so both install side by side", () => {
    const clash = [
      'usr/bin/velocity-desktop',
      'usr/bin/velocity-native-host',
      'usr/lib/Velocity Download Manager/THIRD_PARTY_NOTICES.md',
      'usr/share/applications/Velocity Download Manager.desktop',
      'usr/share/icons/hicolor/128x128/apps/velocity-desktop.png',
    ];
    expect(linuxPackageProblems([...files, 'usr/bin/velocity-aria2c', ...clash])).toEqual([
      'it contains usr/bin/velocity-aria2c: V2 never ships aria2 (ADR-0001)',
      "it installs usr/bin/velocity-aria2c, which V1's package owns",
      ...clash.map((p) => `it installs ${p}, which V1's package owns`),
    ]);
  });

  it('name what is missing', () => {
    expect(linuxPackageProblems(files.filter((f) => !f.endsWith('velocity-torrent') && !f.endsWith('.desktop')))).toEqual([
      'it lacks usr/bin/velocity-torrent',
      'it lacks usr/share/applications/Velocity Download Manager V2.desktop',
    ]);
  });
});

describe('the libraries a packaged program loads', () => {
  const readelf = [
    'Dynamic section at offset 0x2d8 contains 30 entries:',
    '  Tag        Type                         Name/Value',
    ' 0x0000000000000001 (NEEDED)             Shared library: [libstdc++.so.6]',
    ' 0x0000000000000001 (NEEDED)             Shared library: [libc.so.6]',
    ' 0x000000000000000e (SONAME)             Library soname: [libfoo.so.1]',
  ].join('\n');

  it('are read from readelf', () => {
    expect(neededLibraries(readelf)).toEqual(['libstdc++.so.6', 'libc.so.6']);
  });

  it('may be the system and GTK stack', () => {
    expect(neededProblems('usr/bin/velocity-desktop-v2', ['libc.so.6', 'libgtk-3.so.0', 'libwebkit2gtk-4.1.so.0', 'libzstd.so.1'])).toEqual([]);
  });

  it('may not be a system copy of a library V2 links statically', () => {
    expect(neededProblems('usr/bin/velocity-transfer', ['libc.so.6', 'libcurl.so.4', 'libssl.so.3', 'libz.so.1', 'libtorrent-rasterbar.so.2.0'])).toEqual([
      "usr/bin/velocity-transfer loads the system's libcurl.so.4, which V2 links statically",
      "usr/bin/velocity-transfer loads the system's libssl.so.3, which V2 links statically",
      "usr/bin/velocity-transfer loads the system's libz.so.1, which V2 links statically",
      "usr/bin/velocity-transfer loads the system's libtorrent-rasterbar.so.2.0, which V2 links statically",
    ]);
  });
});

describe('the glibc a packaged program needs', () => {
  const objdump = [
    '0000000000000000      DF *UND*  0000000000000000 (GLIBC_2.2.5) memcpy',
    '0000000000000000      DF *UND*  0000000000000000 (GLIBC_2.34)  __libc_start_main',
    '0000000000000000      DF *UND*  0000000000000000 (GLIBC_2.17)  clock_gettime',
    '0000000000000000      DF *UND*  0000000000000000 (GLIBCXX_3.4.29) _ZNSt',
  ].join('\n');

  it('is the newest GLIBC_ version, compared as numbers', () => {
    expect(newestGlibc(objdump)).toBe('2.34');
    expect(newestGlibc('(GLIBC_2.9) a\n(GLIBC_2.10) b')).toBe('2.10');
    expect(newestGlibc('no versions')).toBeNull();
  });

  it("may be up to Ubuntu 22.04's 2.35", () => {
    expect(glibcProblem('usr/bin/velocity-transfer', '2.35')).toBeNull();
    expect(glibcProblem('usr/bin/velocity-transfer', null)).toBeNull();
  });

  it('may not be newer', () => {
    expect(glibcProblem('usr/bin/velocity-transfer', '2.38')).toBe(
      'usr/bin/velocity-transfer needs glibc 2.38, newer than the 2.35 the packages support (Ubuntu 22.04)',
    );
  });
});
