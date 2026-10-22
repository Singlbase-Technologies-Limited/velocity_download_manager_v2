import { existsSync, readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import {
  LIBCURL_FLOOR,
  checkArtifacts,
  checkBuildInfo,
  checkDownload,
  checkLinked,
  checkSignature,
  createBuildInfo,
  librariesFor,
  parseCurlVersion,
  parseLock,
} from './native-lib.mjs';

const CURL_SHA = '8dadd7d2536a314f982ad05a651b713f426bf7d02cc2c76cffb9796f2218e6c5';
const ZLIB_SHA = 'd7a0654783a4da529d1bb793b7ad9c3318020af77667bcae35f95d0e42a792f3';
const CURL_FPR = '27EDEAF22F3ABCEB50DB9A125CC908FDB71E12C2';
const curlLine = `curl 8.23.0 https://curl.se/download/curl-8.23.0.tar.xz ${CURL_SHA} verified:${CURL_FPR} curl`;
const zlibLine = `zlib 1.3.2 https://github.com/madler/zlib/releases/download/v1.3.2/zlib-1.3.2.tar.xz ${ZLIB_SHA} none-published Zlib`;

const FULL_LOCK = [
  zlibLine.replace('none-published', 'verified:5ED46A6721D365587791E2AA783FCD8E58BCAFBA'),
  `nghttp2 1.70.0 https://github.com/nghttp2/nghttp2/releases/download/v1.70.0/nghttp2-1.70.0.tar.xz e05cb1388eaca3830aded4ccf20044b6e1ac1a61411dcca11b0437c4285c8bc2 verified:F4F3B91474D1EB29889BD0EF7E8403D5D673C366 MIT`,
  `openssl 3.5.9 https://github.com/openssl/openssl/releases/download/openssl-3.5.9/openssl-3.5.9.tar.gz 603f5602e2eef00d77fbd429d34dcd5822bb301757a1bc9cdb24c670f1eb859a verified:B146647E45A7B33947AB226B2A2C87D161692D40 Apache-2.0`,
  curlLine,
].join('\n');

describe('parseLock', () => {
  it('reads one entry per line and skips comments and blank lines', () => {
    const lock = parseLock(`# header comment\n\n${curlLine}\n  # indented comment\n${zlibLine}\n`);
    expect(lock).toEqual([
      {
        name: 'curl',
        version: '8.23.0',
        url: 'https://curl.se/download/curl-8.23.0.tar.xz',
        sha256: CURL_SHA,
        signature: { status: 'verified', fingerprint: CURL_FPR },
        licence: 'curl',
      },
      {
        name: 'zlib',
        version: '1.3.2',
        url: 'https://github.com/madler/zlib/releases/download/v1.3.2/zlib-1.3.2.tar.xz',
        sha256: ZLIB_SHA,
        signature: { status: 'none-published' },
        licence: 'Zlib',
      },
    ]);
  });

  it('records an unverifiable signature with its reason', () => {
    const line = curlLine.replace(`verified:${CURL_FPR}`, 'unverifiable:no-key-published');
    expect(parseLock(line)[0].signature).toEqual({ status: 'unverifiable', reason: 'no-key-published' });
  });

  it('accepts CRLF line endings', () => {
    expect(parseLock(`${curlLine}\r\n${zlibLine}\r\n`).map((e) => e.name)).toEqual(['curl', 'zlib']);
  });

  const rejects = [
    ['a line with the wrong number of columns', `curl 8.23.0 https://curl.se/download/curl-8.23.0.tar.xz ${CURL_SHA} curl`, /line 1: expected 6 columns/],
    ['a source that is not HTTPS', curlLine.replace('https://', 'http://'), /line 1: .*must be an https URL/],
    ['a malformed SHA-256', curlLine.replace(CURL_SHA, CURL_SHA.slice(1)), /line 1: .*SHA-256/],
    ['an upper-case SHA-256', curlLine.replace(CURL_SHA, CURL_SHA.toUpperCase()), /line 1: .*SHA-256/],
    ['a source file that does not name the library and version', curlLine.replace('curl-8.23.0.tar.xz', 'curl-8.22.0.tar.xz'), /line 1: .*curl-8\.23\.0/],
    ['a verified signature without a full fingerprint', curlLine.replace(CURL_FPR, CURL_FPR.slice(0, 16)), /line 1: .*signature/],
    ['an unknown signature status', curlLine.replace(`verified:${CURL_FPR}`, 'trusted'), /line 1: .*signature/],
    ['an unverifiable signature without a reason', curlLine.replace(`verified:${CURL_FPR}`, 'unverifiable:'), /line 1: .*signature/],
    ['a licence outside the allowed list', curlLine.replace(/ curl$/, ' LGPL-2.1-only'), /line 1: .*licence LGPL-2\.1-only/],
    ['a malformed version', curlLine.replace('curl 8.23.0', 'curl latest'), /line 1: .*version/],
  ];
  it.each(rejects)('rejects %s', (_, text, message) => {
    expect(() => parseLock(text)).toThrow(message);
  });

  it('rejects a library listed twice', () => {
    expect(() => parseLock(`${curlLine}\n${curlLine}\n`)).toThrow(/line 2: curl is listed twice/);
  });

  it('rejects an empty lock file', () => {
    expect(() => parseLock('# only comments\n')).toThrow(/no libraries/);
  });
});

describe('checkDownload', () => {
  const [curl] = parseLock(curlLine);
  it('accepts a download whose SHA-256 matches the lock file', () => {
    expect(() => checkDownload(curl, CURL_SHA)).not.toThrow();
  });
  it('refuses a download whose SHA-256 differs, naming both values', () => {
    const other = '0'.repeat(64);
    expect(() => checkDownload(curl, other)).toThrow(new RegExp(`curl-8\\.23\\.0\\.tar\\.xz.*${other}.*${CURL_SHA}`));
  });
});

describe('checkSignature', () => {
  const [curl] = parseLock(curlLine);
  // gpg --status-fd output: VALIDSIG <signing key> <date> <time> ... <primary key fingerprint>.
  const validsig = (primary, signing = primary) => `[GNUPG:] NEWSIG\n[GNUPG:] GOODSIG 5CC908FDB71E12C2 Daniel\n[GNUPG:] VALIDSIG ${signing} 2026-10-14 1791957316 0 4 0 1 10 00 ${primary}\n`;

  it('accepts a good signature from the pinned key', () => {
    expect(checkSignature(curl, validsig(CURL_FPR))).toBe(`verified:${CURL_FPR}`);
  });
  it('accepts a signature made by a subkey of the pinned primary key', () => {
    expect(checkSignature(curl, validsig(CURL_FPR, '516B622918D15C478AB1EA3A5339A2BE82E07DEC'))).toBe(`verified:${CURL_FPR}`);
  });
  it('refuses a good signature from a different key', () => {
    expect(() => checkSignature(curl, validsig('F4F3B91474D1EB29889BD0EF7E8403D5D673C366'))).toThrow(/curl: signed by F4F3B9.*expected 27EDEA/);
  });
  it.each([
    ['a bad signature', '[GNUPG:] NEWSIG\n[GNUPG:] BADSIG 5CC908FDB71E12C2 Daniel\n'],
    ['a missing key', '[GNUPG:] NEWSIG\n[GNUPG:] ERRSIG 5CC908FDB71E12C2 1 10 00 1791957316 9 -\n[GNUPG:] NO_PUBKEY 5CC908FDB71E12C2\n'],
    ['an expired key', `${validsig(CURL_FPR)}[GNUPG:] EXPKEYSIG 5CC908FDB71E12C2 Daniel\n`],
    ['a revoked key', `${validsig(CURL_FPR)}[GNUPG:] REVKEYSIG 5CC908FDB71E12C2 Daniel\n`],
    ['no output at all', ''],
  ])('refuses %s', (_, status) => {
    expect(() => checkSignature(curl, status)).toThrow(/curl: /);
  });
  it('refuses a file carrying two signatures, even if one is valid', () => {
    expect(() => checkSignature(curl, validsig(CURL_FPR) + validsig(CURL_FPR))).toThrow(/curl: expected exactly one signature/);
  });
  it('reports, without gpg, a library whose upstream publishes no signature', () => {
    const [zlib] = parseLock(zlibLine);
    expect(checkSignature(zlib, null)).toBe('none-published');
  });
  it('reports an unverifiable signature with its reason', () => {
    const [entry] = parseLock(curlLine.replace(`verified:${CURL_FPR}`, 'unverifiable:no-key-published'));
    expect(checkSignature(entry, null)).toBe('unverifiable:no-key-published');
  });
});

describe('librariesFor', () => {
  it('builds zlib, nghttp2 and curl on Windows, where Schannel is the TLS backend', () => {
    expect(librariesFor('x86_64-pc-windows-msvc')).toEqual({ libraries: ['zlib', 'nghttp2', 'curl'], tls: 'Schannel' });
  });
  it('adds a static OpenSSL on Linux', () => {
    expect(librariesFor('x86_64-unknown-linux-gnu')).toEqual({ libraries: ['zlib', 'nghttp2', 'openssl', 'curl'], tls: 'OpenSSL' });
  });
  it('refuses a target with no established build', () => {
    expect(() => librariesFor('aarch64-apple-darwin')).toThrow(/no native build is established for aarch64-apple-darwin/);
  });
});

// Real `curl -V` output of the Windows and Linux builds on 2026-10-22 (docs/BUILDING_NATIVE.md).
const WINDOWS_CURL_V = [
  'curl 8.23.0 (Windows) libcurl/8.23.0 Schannel zlib/1.3.2 nghttp2/1.70.0',
  'Release-Date: 2026-10-14',
  'Protocols: ftp ftps http https',
  'Features: alt-svc AsynchDNS HSTS HTTP2 HTTPS-proxy IPv6 Kerberos Largefile libz SPNEGO SSL SSPI threadsafe UnixSockets',
  '',
].join('\r\n');
const LINUX_CURL_V = [
  'curl 8.23.0 (x86_64-pc-linux-gnu) libcurl/8.23.0 OpenSSL/3.5.9 zlib/1.3.2 nghttp2/1.70.0',
  'Release-Date: 2026-10-14',
  'Protocols: ftp ftps http https',
  'Features: alt-svc AsynchDNS HSTS HTTP2 HTTPS-proxy IPv6 Largefile libz SSL threadsafe UnixSockets',
  '',
].join('\n');

describe('parseCurlVersion', () => {
  it('reads the linked versions, protocols and features from `curl -V`', () => {
    expect(parseCurlVersion(LINUX_CURL_V)).toEqual({
      libcurl: '8.23.0',
      tls: 'OpenSSL/3.5.9',
      libraries: { zlib: '1.3.2', nghttp2: '1.70.0' },
      protocols: ['ftp', 'ftps', 'http', 'https'],
      features: ['alt-svc', 'AsynchDNS', 'HSTS', 'HTTP2', 'HTTPS-proxy', 'IPv6', 'Largefile', 'libz', 'SSL', 'threadsafe', 'UnixSockets'],
    });
  });
  it('reads a TLS backend that has no version, such as Schannel', () => {
    const v = parseCurlVersion(WINDOWS_CURL_V);
    expect(v.tls).toBe('Schannel');
    expect(v.libraries).toEqual({ zlib: '1.3.2', nghttp2: '1.70.0' });
  });
  it('refuses output that is not from curl', () => {
    expect(() => parseCurlVersion('command not found')).toThrow(/not `curl -V` output/);
  });
});

describe('checkLinked', () => {
  const lock = parseLock(FULL_LOCK);
  it('accepts a Linux build whose linked libraries match the lock file', () => {
    expect(checkLinked('x86_64-unknown-linux-gnu', lock, parseCurlVersion(LINUX_CURL_V))).toEqual([]);
  });
  it('accepts a Windows build linked with Schannel', () => {
    expect(checkLinked('x86_64-pc-windows-msvc', lock, parseCurlVersion(WINDOWS_CURL_V))).toEqual([]);
  });
  it('reports every library whose linked version differs from the lock file', () => {
    const linked = parseCurlVersion(LINUX_CURL_V.replace('libcurl/8.23.0', 'libcurl/8.22.0').replace('zlib/1.3.2', 'zlib/1.3.1'));
    expect(checkLinked('x86_64-unknown-linux-gnu', lock, linked)).toEqual([
      'libcurl 8.22.0 is linked, but the lock file pins curl 8.23.0',
      'libcurl 8.22.0 is below the version floor 8.23.0',
      'zlib 1.3.1 is linked, but the lock file pins 1.3.2',
    ]);
  });
  it('reports a libcurl below the version floor even if the lock file pins it', () => {
    const old = parseLock(FULL_LOCK.replaceAll('8.23.0', '8.22.0'));
    const linked = parseCurlVersion(LINUX_CURL_V.replaceAll('8.23.0', '8.22.0'));
    expect(checkLinked('x86_64-unknown-linux-gnu', old, linked)).toEqual(['libcurl 8.22.0 is below the version floor 8.23.0']);
  });
  it('reports the wrong TLS backend', () => {
    const linked = parseCurlVersion(LINUX_CURL_V.replace('OpenSSL/3.5.9', 'GnuTLS/3.8.3'));
    expect(checkLinked('x86_64-unknown-linux-gnu', lock, linked)).toEqual(['TLS backend GnuTLS/3.8.3 is linked, but the build needs OpenSSL/3.5.9']);
  });
  it('reports missing required protocols and features', () => {
    const linked = parseCurlVersion(LINUX_CURL_V.replace('Protocols: ftp ftps http https', 'Protocols: http https').replace(' HTTP2', ''));
    expect(checkLinked('x86_64-unknown-linux-gnu', lock, linked)).toEqual([
      'required protocols missing: ftp, ftps',
      'required features missing: HTTP2',
    ]);
  });
  it('reports protocols V2 has no use for, which widen what a redirect can reach', () => {
    const linked = parseCurlVersion(LINUX_CURL_V.replace('Protocols: ftp ftps http https', 'Protocols: file ftp ftps http https ldap'));
    expect(checkLinked('x86_64-unknown-linux-gnu', lock, linked)).toEqual(['protocols that should be disabled: file, ldap']);
  });
});

describe('checkBuildInfo', () => {
  const lock = parseLock(FULL_LOCK);
  const info = () =>
    createBuildInfo({
      target: 'x86_64-unknown-linux-gnu',
      lock,
      signatures: { zlib: 'verified:5ED46A6721D365587791E2AA783FCD8E58BCAFBA', nghttp2: 'verified:F4F3B91474D1EB29889BD0EF7E8403D5D673C366', openssl: 'verified:B146647E45A7B33947AB226B2A2C87D161692D40', curl: 'verified:27EDEAF22F3ABCEB50DB9A125CC908FDB71E12C2' },
      options: { zlib: ['--static'], nghttp2: ['--enable-lib-only'], openssl: ['no-shared'], curl: ['--disable-shared'] },
      linked: parseCurlVersion(LINUX_CURL_V),
      artifacts: [{ path: 'lib/libcurl.a', sha256: 'a'.repeat(64) }],
      toolchains: { rustc: 'rustc 1.99.1', node: 'v24.12.0', cc: 'gcc 13.3.0' },
      v2: { commit: 'b'.repeat(40), dirty: false },
      cacheKey: 'c'.repeat(64),
    });

  it('accepts a complete record that matches the lock file', () => {
    expect(checkBuildInfo(info(), lock)).toEqual([]);
  });
  it('records each library with its lock-file source, signature result and build options', () => {
    expect(info().libraries.find((l) => l.name === 'curl')).toEqual({
      name: 'curl',
      version: '8.23.0',
      url: 'https://curl.se/download/curl-8.23.0.tar.xz',
      sha256: CURL_SHA,
      signature: 'verified:27EDEAF22F3ABCEB50DB9A125CC908FDB71E12C2',
      licence: 'curl',
      options: ['--disable-shared'],
    });
  });
  it('reports a record whose sources differ from the lock file', () => {
    const changed = info();
    changed.libraries.find((l) => l.name === 'zlib').sha256 = '0'.repeat(64);
    expect(checkBuildInfo(changed, lock)).toEqual(['zlib: the recorded source SHA-256 differs from the lock file']);
  });
  it.each([
    ['the V2 commit', (i) => delete i.v2],
    ['the toolchains', (i) => delete i.toolchains],
    ['the built artefacts', (i) => (i.artifacts = [])],
    ['a library the target needs', (i) => (i.libraries = i.libraries.filter((l) => l.name !== 'nghttp2'))],
    ['a library signature result', (i) => delete i.libraries[0].signature],
  ])('reports an incomplete record missing %s', (_, mutate) => {
    const broken = info();
    mutate(broken);
    expect(checkBuildInfo(broken, lock).length).toBeGreaterThan(0);
  });
  it('reports a record with an unknown schema version', () => {
    expect(checkBuildInfo({ ...info(), schema: 2 }, lock)).toEqual(['unknown build-info schema 2 (this checker reads schema 1)']);
  });
  it('applies the linked-library checks to the recorded `curl -V` result', () => {
    const old = info();
    old.linked.libcurl = '8.22.0';
    expect(checkBuildInfo(old, lock)).toContain('libcurl 8.22.0 is linked, but the lock file pins curl 8.23.0');
  });
});

describe('the committed lock file', () => {
  const dir = new URL('.', import.meta.url);
  const lock = parseLock(readFileSync(new URL('deps.lock', dir), 'utf8'));
  it('pins every library each established target builds', () => {
    for (const target of ['x86_64-pc-windows-msvc', 'x86_64-unknown-linux-gnu']) {
      for (const name of librariesFor(target).libraries) expect(lock.map((e) => e.name)).toContain(name);
    }
  });
  it('pins a libcurl at or above the version floor', () => {
    const curl = lock.find((e) => e.name === 'curl');
    expect(checkLinked('x86_64-unknown-linux-gnu', lock, parseCurlVersion(LINUX_CURL_V.replaceAll('8.23.0', curl.version)))).not.toContain(
      `libcurl ${curl.version} is below the version floor ${LIBCURL_FLOOR}`,
    );
  });
  it('has the pinned key of every library whose signature it verifies', () => {
    for (const e of lock.filter((l) => l.signature.status === 'verified')) expect(existsSync(new URL(`keys/${e.name}.asc`, dir))).toBe(true);
  });
});

describe('checkArtifacts', () => {
  const info = { artifacts: [{ path: 'lib/libcurl.a', sha256: 'a'.repeat(64) }, { path: 'bin/curl', sha256: 'b'.repeat(64) }] };
  it('accepts artefacts that still have their recorded SHA-256', () => {
    const hashes = { 'lib/libcurl.a': 'a'.repeat(64), 'bin/curl': 'b'.repeat(64) };
    expect(checkArtifacts(info, (p) => hashes[p])).toEqual([]);
  });
  it('reports an artefact that changed after the build', () => {
    const hashes = { 'lib/libcurl.a': 'c'.repeat(64), 'bin/curl': 'b'.repeat(64) };
    expect(checkArtifacts(info, (p) => hashes[p])).toEqual(['lib/libcurl.a changed after the build: its SHA-256 differs from build-info.json']);
  });
  it('reports a missing artefact', () => {
    expect(checkArtifacts(info, (p) => (p === 'bin/curl' ? 'b'.repeat(64) : null))).toEqual(['lib/libcurl.a is missing']);
  });
  it('refuses an artefact path that leaves the build folder', () => {
    const escaping = { artifacts: [{ path: '../../../etc/passwd', sha256: 'a'.repeat(64) }] };
    expect(checkArtifacts(escaping, () => 'a'.repeat(64))).toEqual(['../../../etc/passwd is not a path inside the build folder']);
  });
  it('refuses a Windows-style path that leaves the build folder', () => {
    const escaping = { artifacts: [{ path: 'lib\\..\\..\\secret.lib', sha256: 'a'.repeat(64) }] };
    expect(checkArtifacts(escaping, () => 'a'.repeat(64))).toEqual(['lib\\..\\..\\secret.lib is not a path inside the build folder']);
  });
});
