import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import {
  ALLOWED_EXCEPTIONS,
  BASELINE_LIBRARIES,
  chooseLicence,
  crateComponents,
  firstDifference,
  FILE_COPYLEFT,
  FONT_LICENCES,
  licenceProblems,
  linkTarget,
  nativeComponents,
  normaliseText,
  npmComponents,
  parseDepsLock,
  parseSpdx,
  PERMISSIVE,
  pickLicenceFiles,
  renderNotices,
  VCPKG_BASELINE,
} from './notices-lib.mjs';

const crate = (name, licence) => ({ name, version: '1.0.0', licence });

describe('parseSpdx', () => {
  it('reads OR, AND, WITH and parentheses, AND binding tighter than OR', () => {
    expect(parseSpdx('(MIT OR Apache-2.0) AND Unicode-3.0')).toEqual({
      op: 'AND',
      terms: [{ op: 'OR', terms: [{ id: 'MIT' }, { id: 'Apache-2.0' }] }, { id: 'Unicode-3.0' }],
    });
    expect(parseSpdx('MIT OR Apache-2.0 AND ISC')).toEqual({
      op: 'OR',
      terms: [{ id: 'MIT' }, { op: 'AND', terms: [{ id: 'Apache-2.0' }, { id: 'ISC' }] }],
    });
    expect(parseSpdx('Apache-2.0 WITH LLVM-exception')).toEqual({ id: 'Apache-2.0', exception: 'LLVM-exception' });
  });

  it("reads the older slash form some crates use as OR", () => {
    expect(parseSpdx('MIT/Apache-2.0')).toEqual({ op: 'OR', terms: [{ id: 'MIT' }, { id: 'Apache-2.0' }] });
  });

  it('refuses an expression it can\'t read', () => {
    expect(() => parseSpdx('')).toThrow();
    expect(() => parseSpdx('(MIT OR Apache-2.0')).toThrow();
    expect(() => parseSpdx('MIT OR')).toThrow();
    expect(() => parseSpdx('MIT Apache-2.0')).toThrow();
  });
});

describe('chooseLicence', () => {
  it('uses a choice under the licence V2 prefers', () => {
    expect(chooseLicence('Apache-2.0 OR MIT', crate('a'))).toEqual({ ok: true, ids: ['MIT'] });
    expect(chooseLicence('Zlib OR Apache-2.0 OR MIT', crate('a'))).toEqual({ ok: true, ids: ['MIT'] });
  });

  it('uses a component that offers a copyleft option under a permissive one', () => {
    expect(chooseLicence('MIT OR Apache-2.0 OR LGPL-2.1-or-later', crate('r-efi'))).toEqual({ ok: true, ids: ['MIT'] });
  });

  it('needs every part of an AND', () => {
    expect(chooseLicence('(MIT OR Apache-2.0) AND Unicode-3.0', crate('a'))).toEqual({ ok: true, ids: ['MIT', 'Unicode-3.0'] });
    expect(chooseLicence('MIT AND GPL-3.0-only', crate('a')).ok).toBe(false);
  });

  it('refuses copyleft, an unknown licence and a custom one', () => {
    for (const licence of ['GPL-3.0-only', 'LGPL-2.1-only', 'AGPL-3.0-or-later', 'LicenseRef-proprietary', 'SSPL-1.0']) {
      expect(chooseLicence(licence, crate('a'))).toMatchObject({ ok: false });
    }
  });

  it('allows MPL-2.0, file-level copyleft used unmodified', () => {
    expect(chooseLicence('MPL-2.0', crate('cssparser'))).toEqual({ ok: true, ids: ['MPL-2.0'] });
  });

  it('allows OFL-1.1 only for a font package', () => {
    expect(chooseLicence('OFL-1.1', crate('@fontsource-variable/inter')).ok).toBe(true);
    expect(chooseLicence('OFL-1.1', crate('some-library')).ok).toBe(false);
  });

  it('allows only the LLVM exception, and only with Apache-2.0', () => {
    expect(chooseLicence('Apache-2.0 WITH LLVM-exception', crate('a'))).toEqual({ ok: true, ids: ['Apache-2.0'] });
    expect(chooseLicence('GPL-2.0-only WITH Classpath-exception-2.0', crate('a')).ok).toBe(false);
  });
});

describe('licenceProblems', () => {
  it('names each component that may not ship, and passes the rest', () => {
    const problems = licenceProblems([
      crate('fine', 'MIT OR Apache-2.0'),
      crate('copyleft', 'GPL-2.0-or-later'),
      crate('undeclared', ''),
    ]);
    expect(problems).toHaveLength(2);
    expect(problems[0]).toMatch(/^copyleft 1\.0\.0 \(GPL-2\.0-or-later\): GPL-2\.0-or-later is not in the allowed list/);
    expect(problems[1]).toMatch(/^undeclared 1\.0\.0 \(none\): no licence declared/);
  });

  it('refuses an unapproved copyleft native library', () => {
    const native = nativeComponents(
      parseDepsLock('libfoo 1.0 https://example.org/libfoo-1.0.tar.xz abc verified:F00 LGPL-2.1-only\n'),
      [],
    );
    expect(licenceProblems(native)).toEqual([expect.stringMatching(/^libfoo 1\.0 \(LGPL-2\.1-only\)/)]);
  });
});

describe('the allowed list', () => {
  // docs/DEPENDENCIES.md is the policy's one home; this keeps the code and the table equal.
  const policy = readFileSync(new URL('../docs/DEPENDENCIES.md', import.meta.url), 'utf8');
  const row = (cls) => policy.split('\n').find((l) => l.startsWith(`| **${cls}`));

  it('matches DEPENDENCIES.md, licence by licence', () => {
    const permissive = row('Permissive: allowed');
    for (const id of PERMISSIVE.filter((id) => id !== 'curl')) expect(permissive).toContain(id);
    expect(permissive).toContain('the curl licence');
    expect(permissive).toContain('LLVM-exception');
    expect(ALLOWED_EXCEPTIONS).toEqual(['LLVM-exception']);
    for (const id of FILE_COPYLEFT) expect(row('File-level copyleft: allowed unmodified')).toContain(id);
    for (const id of FONT_LICENCES) expect(row('Fonts: allowed unmodified')).toContain(id);
  });

  it('lists every permissive licence DEPENDENCIES.md names', () => {
    const named = row('Permissive: allowed').split('|')[2].replace(/\(with or without LLVM-exception\)|the curl licence/g, '').split(',').map((s) => s.trim()).filter(Boolean);
    expect([...named].sort()).toEqual(PERMISSIVE.filter((id) => id !== 'curl').sort());
  });
});

describe('native libraries', () => {
  it('reads deps.lock and adds the vcpkg ports and Boost, OpenSSL once', () => {
    const lock = parseDepsLock(
      '# comment\nopenssl 3.5.9 https://example.org/openssl.tar.gz abc verified:F00 Apache-2.0\ncurl 8.23.0 https://curl.se/curl.tar.xz def verified:BA4 curl\n',
    );
    const native = nativeComponents(lock, [
      { name: 'libtorrent', version: '2.0.15' },
      { name: 'openssl', version: '3.5.9' },
    ]);
    expect(native.map((c) => `${c.name} ${c.version} ${c.licence}`)).toEqual([
      'openssl 3.5.9 Apache-2.0',
      'curl 8.23.0 curl',
      'libtorrent 2.0.15 BSD-3-Clause',
      'boost 1.91.0 BSL-1.0',
    ]);
  });

  it('refuses a library pinned at two versions, or a port without a recorded licence', () => {
    const lock = parseDepsLock('openssl 3.5.9 https://example.org/o.tar.gz abc verified:F00 Apache-2.0\n');
    expect(() => nativeComponents(lock, [{ name: 'openssl', version: '3.6.0' }])).toThrow(/3\.5\.9 in deps\.lock but 3\.6\.0/);
    expect(() => nativeComponents([], [{ name: 'libfoo', version: '1' }])).toThrow(/no licence recorded/);
  });

  it('refuses a deps.lock line without a licence', () => {
    expect(() => parseDepsLock('zlib 1.3.2 https://example.org/z.tar.xz abc\n')).toThrow(/six fields/);
  });

  it('reads the real pins', () => {
    const lock = parseDepsLock(readFileSync(new URL('./native/deps.lock', import.meta.url), 'utf8'));
    expect(lock.map((c) => c.name).sort()).toEqual(['curl', 'nghttp2', 'openssl', 'zlib']);
  });

  it('checks Boost again when the vcpkg baseline moves', () => {
    // Boost's version comes with the baseline, not a pin: a new baseline means a new check of it.
    const manifest = JSON.parse(readFileSync(new URL('./native/vcpkg/vcpkg.json', import.meta.url), 'utf8'));
    expect(manifest['builtin-baseline']).toBe(VCPKG_BASELINE);
    expect(BASELINE_LIBRARIES.map((c) => c.name)).toEqual(['boost']);
  });
});

describe('crateComponents', () => {
  it('lists the crates the tree names, without the workspace crates', () => {
    const metadata = {
      workspace_members: ['path+file:///w/core#velocity-core@0.1.0'],
      packages: [
        { id: 'path+file:///w/core#velocity-core@0.1.0', name: 'velocity-core', version: '0.1.0', license: null, manifest_path: '/w/core/Cargo.toml' },
        { id: 'registry+x#serde@1.0.0', name: 'serde', version: '1.0.0', license: 'MIT OR Apache-2.0', repository: 'https://github.com/serde-rs/serde', authors: ['D'], manifest_path: '/r/serde-1.0.0/Cargo.toml' },
        { id: 'registry+x#cc@1.0.0', name: 'cc', version: '1.0.0', license: 'MIT', manifest_path: '/r/cc-1.0.0/Cargo.toml' },
      ],
    };
    const tree = 'velocity-core v0.1.0 (/w/core)\nserde v1.0.0\nserde v1.0.0\n';
    expect(crateComponents(metadata, tree)).toEqual([
      { name: 'serde', version: '1.0.0', licence: 'MIT OR Apache-2.0', source: 'https://github.com/serde-rs/serde', authors: ['D'], dir: '/r/serde-1.0.0' },
    ]);
  });
});

describe('npmComponents', () => {
  it('lists each version of each package with its directory', () => {
    const list = { MIT: [{ name: 'react', versions: ['19.3.1'], paths: ['/n/react'], license: 'MIT', author: 'Meta', homepage: 'https://react.dev/' }] };
    expect(npmComponents(list)).toEqual([
      { name: 'react', version: '19.3.1', licence: 'MIT', source: 'https://react.dev/', authors: ['Meta'], dir: '/n/react' },
    ]);
  });
});

describe('pickLicenceFiles', () => {
  const files = ['Cargo.toml', 'LICENSE-APACHE', 'LICENSE-MIT', 'COPYRIGHT', 'NOTICE', 'LICENSE.spdx', 'README.md'];

  it('takes the files of the licences used, and those naming none', () => {
    expect(pickLicenceFiles(files, ['MIT'])).toEqual(['COPYRIGHT', 'LICENSE-MIT', 'NOTICE']);
    expect(pickLicenceFiles(files, ['Apache-2.0'])).toEqual(['COPYRIGHT', 'LICENSE-APACHE', 'NOTICE']);
  });

  it('keeps a single LICENSE whatever it holds', () => {
    expect(pickLicenceFiles(['LICENSE', 'src'], ['MIT'])).toEqual(['LICENSE']);
  });

  it('tells 0BSD from the BSD licences', () => {
    expect(pickLicenceFiles(['LICENSE-0BSD', 'LICENSE-BSD', 'LICENSE-MIT'], ['0BSD'])).toEqual(['LICENSE-0BSD']);
    expect(pickLicenceFiles(['LICENSE-0BSD', 'LICENSE-BSD'], ['BSD-3-Clause'])).toEqual(['LICENSE-BSD']);
  });
});

describe('linkTarget', () => {
  it('reads a link that git checked out as a file holding its target', () => {
    expect(linkTarget('../LICENSE')).toBe('../LICENSE');
    expect(linkTarget('../LICENSE-MIT\n')).toBe('../LICENSE-MIT');
    expect(linkTarget('..\\COPYING')).toBe('..\\COPYING');
  });

  it('takes anything else as the text itself', () => {
    expect(linkTarget('MIT License')).toBeNull();
    expect(linkTarget('LICENSE')).toBeNull();
    expect(linkTarget('../README.md')).toBeNull();
    expect(linkTarget('/etc/LICENSE')).toBeNull();
    expect(linkTarget('C:/LICENSE')).toBeNull();
    expect(linkTarget('Copyright (c) 2014 Carl Lerche\n\nPermission is hereby granted')).toBeNull();
  });
});

describe('firstDifference', () => {
  it('finds the first line that differs, with what follows on each side', () => {
    expect(firstDifference('a\nb\nc\nd', 'a\nb\nX\nd', 2)).toEqual({ line: 3, committed: ['c', 'd'], fresh: ['X', 'd'] });
  });

  it('reports a file that only got longer, and nothing for equal files', () => {
    expect(firstDifference('a\nb', 'a\nb\nc')).toEqual({ line: 3, committed: [], fresh: ['c'] });
    expect(firstDifference('a\nb', 'a\nb')).toBeNull();
  });
});

describe('normaliseText', () => {
  it('ignores line endings and trailing spaces', () => {
    expect(normaliseText('MIT  \r\nLicense\r\n\r\n')).toBe(normaliseText('MIT\nLicense'));
  });
});

describe('renderNotices', () => {
  const mit = 'MIT License\n\nCopyright (c) A';
  const input = {
    native: [{ name: 'zlib', version: '1.3.2', licence: 'Zlib', source: 'https://zlib.net/', authors: [], texts: [{ licence: 'Zlib', text: 'zlib text' }] }],
    crates: [
      { name: 'b', version: '1.0.0', licence: 'MIT OR Apache-2.0', source: 'https://b', authors: [], texts: [{ licence: 'MIT', text: mit }] },
      { name: 'a', version: '2.0.0', licence: 'MIT', source: 'https://a', authors: [], texts: [{ licence: 'MIT', text: `${mit}\n` }] },
      { name: 'c', version: '0.1.0', licence: 'Apache-2.0', source: 'https://c', authors: ['C Author <c@example.org>'], texts: [] },
    ],
    npm: [{ name: 'p|q', version: '1.0.0', licence: 'ISC', source: 'https://p', authors: [], texts: [{ licence: 'ISC', text: 'isc text' }] }],
    standardTexts: { 'Apache-2.0': 'Apache standard text' },
  };
  const out = renderNotices(input);

  it('lists every component sorted, with the licence it is used under', () => {
    expect(out).toContain('| a | 2.0.0 | MIT | MIT | https://a |\n| b | 1.0.0 | MIT OR Apache-2.0 | MIT | https://b |');
    expect(out).toContain('## Rust crates (3)');
    expect(out).toContain('| p\\|q | 1.0.0 | ISC | ISC | https://p |');
  });

  it('prints an identical text once, naming every component that ships it', () => {
    expect(out.split('MIT License').length - 1).toBe(1);
    expect(out).toContain('Used by: a 2.0.0, b 1.0.0.');
  });

  it("gives a component without a licence file its licence's standard text and its authors, without emails", () => {
    expect(out).toContain('### Apache-2.0, standard text');
    expect(out).toContain('- c 0.1.0, by C Author\n');
    expect(out).toContain('Apache standard text');
    expect(out).not.toContain('c@example.org');
  });

  it("refuses to render without a standard text it needs", () => {
    expect(() => renderNotices({ ...input, standardTexts: {} })).toThrow(/no standard text for Apache-2\.0/);
  });

  it('is the same for the same input', () => {
    expect(renderNotices(input)).toBe(out);
  });
});
