import { describe, expect, it } from 'vitest';
import { LIBTORRENT_VERSION, TRIPLETS, checkLibtorrentInfo, librariesForLibtorrent, parsePackageVersion, parseTargets } from './libtorrent-lib.mjs';

// The two lines that matter, as vcpkg's libtorrent 2.0.15 package writes them (#62).
const WINDOWS_TARGETS = String.raw`
set_target_properties(LibtorrentRasterbar::torrent-rasterbar PROPERTIES
  INTERFACE_COMPILE_DEFINITIONS "\$<\$<CONFIG:Debug>:TORRENT_USE_ASSERTS>;BOOST_ASIO_ENABLE_CANCELIO;BOOST_ASIO_NO_DEPRECATED;WIN32_LEAN_AND_MEAN;BOOST_ALL_NO_LIB;_SCL_SECURE_NO_DEPRECATE;_CRT_SECURE_NO_DEPRECATE;TORRENT_NO_DEPRECATE;TORRENT_USE_OPENSSL;TORRENT_USE_LIBCRYPTO;TORRENT_SSL_PEERS;OPENSSL_NO_SSL2;OPENSSL_NO_SSL3;OPENSSL_NO_TLS1;OPENSSL_NO_TLS1_1;OPENSSL_NO_DTLS1"
  INTERFACE_LINK_LIBRARIES "Threads::Threads;bcrypt;mswsock;ws2_32;iphlpapi;\$<\$<NOT:\$<CONFIG:DEBUG>>:crypt32>;\$<\$<CONFIG:DEBUG>:dbghelp>;\$<\$<NOT:\$<CONFIG:DEBUG>>:OpenSSL::SSL>;\$<\$<CONFIG:DEBUG>:crypt32>;\$<\$<NOT:\$<CONFIG:DEBUG>>:Boost::headers>;\$<\$<CONFIG:DEBUG>:OpenSSL::SSL>;\$<\$<CONFIG:DEBUG>:Boost::headers>"
)
`;
const LINUX_TARGETS = String.raw`
set_target_properties(LibtorrentRasterbar::torrent-rasterbar PROPERTIES
  INTERFACE_COMPILE_DEFINITIONS "\$<\$<CONFIG:Debug>:TORRENT_USE_ASSERTS>;BOOST_ASIO_ENABLE_CANCELIO;BOOST_ASIO_NO_DEPRECATED;TORRENT_NO_DEPRECATE;TORRENT_USE_OPENSSL;TORRENT_USE_LIBCRYPTO;TORRENT_SSL_PEERS;OPENSSL_NO_SSL2;OPENSSL_NO_SSL3;OPENSSL_NO_TLS1;OPENSSL_NO_TLS1_1;OPENSSL_NO_DTLS1"
  INTERFACE_LINK_LIBRARIES "Threads::Threads;OpenSSL::SSL;Boost::headers"
)
`;
const COMMON_DEFINES = ['BOOST_ASIO_ENABLE_CANCELIO', 'BOOST_ASIO_NO_DEPRECATED', 'TORRENT_NO_DEPRECATE', 'TORRENT_USE_OPENSSL', 'TORRENT_USE_LIBCRYPTO', 'TORRENT_SSL_PEERS', 'OPENSSL_NO_SSL2', 'OPENSSL_NO_SSL3', 'OPENSSL_NO_TLS1', 'OPENSSL_NO_TLS1_1', 'OPENSSL_NO_DTLS1'];

describe('parseTargets', () => {
  it('keeps the release definitions and drops the debug-only ones', () => {
    const { defines } = parseTargets(LINUX_TARGETS);
    expect(defines).toEqual(COMMON_DEFINES);
    expect(defines).not.toContain('TORRENT_USE_ASSERTS');
  });

  it('keeps Windows-only definitions in the order libtorrent lists them', () => {
    const { defines } = parseTargets(WINDOWS_TARGETS);
    expect(defines.slice(0, 5)).toEqual(['BOOST_ASIO_ENABLE_CANCELIO', 'BOOST_ASIO_NO_DEPRECATED', 'WIN32_LEAN_AND_MEAN', 'BOOST_ALL_NO_LIB', '_SCL_SECURE_NO_DEPRECATE']);
    expect(defines).toHaveLength(15);
  });

  it('unwraps release-only libraries and drops debug-only ones', () => {
    expect(parseTargets(WINDOWS_TARGETS).links).toEqual(['Threads::Threads', 'bcrypt', 'mswsock', 'ws2_32', 'iphlpapi', 'crypt32', 'OpenSSL::SSL', 'Boost::headers']);
    expect(parseTargets(LINUX_TARGETS).links).toEqual(['Threads::Threads', 'OpenSSL::SSL', 'Boost::headers']);
  });

  it('refuses a package without the libtorrent target properties', () => {
    expect(() => parseTargets('set_target_properties(Other PROPERTIES)')).toThrow(/INTERFACE_COMPILE_DEFINITIONS/);
    expect(() => parseTargets(LINUX_TARGETS.replace(/ {2}INTERFACE_LINK_LIBRARIES.*\n/, ''))).toThrow(/INTERFACE_LINK_LIBRARIES/);
  });

  it('refuses a generator expression it does not understand, rather than guessing', () => {
    const odd = LINUX_TARGETS.replace('OpenSSL::SSL', String.raw`\$<\$<PLATFORM_ID:Linux>:OpenSSL::SSL>`);
    expect(() => parseTargets(odd)).toThrow(/PLATFORM_ID/);
  });
});

describe('librariesForLibtorrent', () => {
  it('maps the Windows targets to static and system libraries', () => {
    expect(librariesForLibtorrent('x86_64-pc-windows-msvc', parseTargets(WINDOWS_TARGETS).links)).toEqual({
      staticLibs: ['torrent-rasterbar', 'libssl', 'libcrypto'],
      systemLibs: ['bcrypt', 'mswsock', 'ws2_32', 'iphlpapi', 'crypt32', 'advapi32', 'user32'],
    });
  });

  it('maps the Linux targets, with the threads and dl libraries a static OpenSSL needs', () => {
    expect(librariesForLibtorrent('x86_64-unknown-linux-gnu', parseTargets(LINUX_TARGETS).links)).toEqual({
      staticLibs: ['torrent-rasterbar', 'ssl', 'crypto'],
      systemLibs: ['pthread', 'dl'],
    });
  });

  it('refuses an imported target it has no mapping for', () => {
    expect(() => librariesForLibtorrent('x86_64-unknown-linux-gnu', ['Threads::Threads', 'ZLIB::ZLIB'])).toThrow(/ZLIB::ZLIB/);
  });

  it('refuses a target with no established build', () => {
    expect(() => librariesForLibtorrent('aarch64-apple-darwin', [])).toThrow(/aarch64-apple-darwin/);
    expect(Object.keys(TRIPLETS)).toEqual(['x86_64-pc-windows-msvc', 'x86_64-unknown-linux-gnu']);
  });
});

describe('parsePackageVersion', () => {
  it('reads the version from the CMake version file', () => {
    expect(parsePackageVersion('set(PACKAGE_VERSION "2.0.15")\n')).toBe('2.0.15');
    expect(() => parsePackageVersion('nothing here')).toThrow(/PACKAGE_VERSION/);
  });
});

describe('checkLibtorrentInfo', () => {
  const good = {
    schema: 1,
    target: 'x86_64-unknown-linux-gnu',
    libtorrent: LIBTORRENT_VERSION,
    triplet: 'x64-linux-release',
    vcpkgBaseline: '9e593bb18ea69cc5095e012465dcd675a822ed0d',
    prefix: '/home/v/target/native/vcpkg-installed/x64-linux-release',
    defines: COMMON_DEFINES,
    staticLibs: ['torrent-rasterbar', 'ssl', 'crypto'],
    systemLibs: ['pthread', 'dl'],
    cacheKey: 'a'.repeat(64),
  };

  it('accepts a complete record', () => {
    expect(checkLibtorrentInfo(good)).toEqual([]);
  });

  it('names every problem in an incomplete or wrong record', () => {
    const bad = { ...good, schema: 2, libtorrent: '2.0.11', triplet: 'x64-linux', defines: [], vcpkgBaseline: 'main' };
    const problems = checkLibtorrentInfo(bad);
    expect(problems).toHaveLength(5);
    expect(problems.join('\n')).toMatch(/schema/);
    expect(problems.join('\n')).toMatch(/2\.0\.11/);
    expect(problems.join('\n')).toMatch(/x64-linux-release/);
    expect(problems.join('\n')).toMatch(/defines/);
    expect(problems.join('\n')).toMatch(/vcpkgBaseline/);
  });

  it('requires the ABI-relevant definitions libtorrent was built with', () => {
    const problems = checkLibtorrentInfo({ ...good, defines: COMMON_DEFINES.filter((d) => d !== 'TORRENT_SSL_PEERS') });
    expect(problems).toEqual([expect.stringMatching(/TORRENT_SSL_PEERS/)]);
  });
});
