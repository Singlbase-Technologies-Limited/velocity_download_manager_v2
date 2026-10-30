// Pure logic behind `pnpm notices` and `pnpm check:notices` (#76): which licences V2 may ship,
// which licence each component is used under, and THIRD_PARTY_NOTICES.md itself. No I/O here, so
// every rule is unit tested (notices-lib.test.mjs); notices.mjs gathers the inputs.
//
// The allowed licences are docs/DEPENDENCIES.md's, "Allowed licences"; a test keeps the two equal.

/**
 * Permissive licences, allowed and credited, in the order V2 prefers them when a component offers a
 * choice (`MIT OR Apache-2.0` is used under MIT).
 */
export const PERMISSIVE = [
  'MIT',
  'Apache-2.0',
  'BSD-3-Clause',
  'BSD-2-Clause',
  'ISC',
  'Zlib',
  'BSL-1.0',
  'Unicode-3.0',
  '0BSD',
  'MIT-0',
  'Unlicense',
  'CC0-1.0',
  'BlueOak-1.0.0',
  'curl',
];
/** Allowed only with Apache-2.0, as DEPENDENCIES.md says ("with or without LLVM-exception"). */
export const ALLOWED_EXCEPTIONS = ['LLVM-exception'];
/** File-level copyleft, allowed unmodified. */
export const FILE_COPYLEFT = ['MPL-2.0'];
/** Allowed only for a font used as it is published: the npm packages below (PO decision on #29). */
export const FONT_LICENCES = ['OFL-1.1'];
const FONT_PACKAGE = /^@fontsource(-variable)?\//;
/**
 * Components the PO allowed under a licence the rules above refuse, each with its decision. Empty:
 * none has been needed. An entry also needs a licensing note in docs/LICENSING.md.
 * @type {{name: string, licence: string, decision: string}[]}
 */
export const APPROVED_EXCEPTIONS = [];

const RANK = new Map([...PERMISSIVE, ...FILE_COPYLEFT, ...FONT_LICENCES].map((id, i) => [id, i]));

/**
 * Parses an SPDX licence expression into a tree. Accepts the older `MIT/Apache-2.0` form that some
 * crates still use, as `OR`. Throws on anything it can't read.
 */
export function parseSpdx(expression) {
  const tokens = expression
    .replaceAll('/', ' OR ')
    .replaceAll('(', ' ( ')
    .replaceAll(')', ' ) ')
    .split(/\s+/)
    .filter(Boolean);
  if (tokens.length === 0) throw new Error('empty licence expression');
  let at = 0;
  const peek = () => tokens[at];
  const next = () => tokens[at++];
  const binary = (op, operand) => () => {
    const terms = [operand()];
    while (peek() === op) {
      next();
      terms.push(operand());
    }
    return terms.length === 1 ? terms[0] : { op, terms };
  };
  const primary = () => {
    const token = next();
    if (token === undefined) throw new Error(`incomplete licence expression: ${expression}`);
    if (token === '(') {
      const inner = or();
      if (next() !== ')') throw new Error(`unbalanced parentheses: ${expression}`);
      return inner;
    }
    if (['AND', 'OR', 'WITH', ')'].includes(token)) throw new Error(`unexpected "${token}": ${expression}`);
    if (peek() === 'WITH') {
      next();
      const exception = next();
      if (exception === undefined) throw new Error(`missing exception: ${expression}`);
      return { id: token, exception };
    }
    return { id: token };
  };
  const and = binary('AND', primary);
  const or = binary('OR', and);
  const tree = or();
  if (at !== tokens.length) throw new Error(`unexpected "${tokens[at]}": ${expression}`);
  return tree;
}

/**
 * The licences V2 uses a component under, or why it can't ship it.
 * @param {string} expression the component's SPDX expression
 * @param {{name: string}} component used for the font rule and the PO's exceptions
 * @returns {{ok: true, ids: string[]} | {ok: false, reason: string}}
 */
export function chooseLicence(expression, component) {
  let tree;
  try {
    tree = parseSpdx(expression);
  } catch (e) {
    return { ok: false, reason: e.message };
  }
  const exception = APPROVED_EXCEPTIONS.find((x) => x.name === component.name && x.licence === expression);
  if (exception) return { ok: true, ids: leaves(tree) };
  return choose(tree, component);
}

function leaves(tree) {
  return tree.terms ? tree.terms.flatMap(leaves) : [tree.id];
}

function choose(tree, component) {
  if (tree.op === 'OR') {
    const options = tree.terms.map((t) => choose(t, component));
    const allowed = options.filter((o) => o.ok);
    if (allowed.length === 0) return { ok: false, reason: options.map((o) => o.reason).join('; ') };
    const worst = (o) => Math.max(...o.ids.map((id) => RANK.get(id)));
    return allowed.reduce((best, o) => (worst(o) < worst(best) ? o : best));
  }
  if (tree.op === 'AND') {
    const parts = tree.terms.map((t) => choose(t, component));
    const refused = parts.filter((p) => !p.ok);
    if (refused.length) return { ok: false, reason: refused.map((p) => p.reason).join('; ') };
    return { ok: true, ids: [...new Set(parts.flatMap((p) => p.ids))] };
  }
  if (tree.exception !== undefined) {
    return tree.id === 'Apache-2.0' && ALLOWED_EXCEPTIONS.includes(tree.exception)
      ? { ok: true, ids: [tree.id] }
      : { ok: false, reason: `${tree.id} WITH ${tree.exception} is not in the allowed list` };
  }
  if (PERMISSIVE.includes(tree.id) || FILE_COPYLEFT.includes(tree.id)) return { ok: true, ids: [tree.id] };
  if (FONT_LICENCES.includes(tree.id)) {
    return FONT_PACKAGE.test(component.name)
      ? { ok: true, ids: [tree.id] }
      : { ok: false, reason: `${tree.id} is allowed only for a font package` };
  }
  return { ok: false, reason: `${tree.id} is not in the allowed list (docs/DEPENDENCIES.md); it needs the PO's approval` };
}

/** Reads `scripts/native/deps.lock`: name, version, source URL and licence of each native library. */
export function parseDepsLock(text) {
  return text
    .split('\n')
    .map((l) => l.trim())
    .filter((l) => l && !l.startsWith('#'))
    .map((l) => {
      const [name, version, url, , , licence] = l.split(/\s+/);
      if (!licence) throw new Error(`deps.lock line has fewer than six fields: ${l}`);
      return { name, version, licence, source: url };
    });
}

/**
 * Libraries vcpkg builds that no file in the repository pins by version: Boost comes with the
 * vcpkg baseline. Checked against the baseline below on 2026-10-30 (vcpkg's installed status:
 * boost-* 1.91.0); a test fails when the baseline changes, so the version is checked again.
 */
export const VCPKG_BASELINE = '9e593bb18ea69cc5095e012465dcd675a822ed0d';
export const BASELINE_LIBRARIES = [{ name: 'boost', version: '1.91.0', licence: 'BSL-1.0', source: 'https://www.boost.org/' }];
/** Where each vcpkg overlay port's sources come from, for the notices. */
const PORT_SOURCES = {
  libtorrent: 'https://github.com/arvidn/libtorrent',
  openssl: 'https://github.com/openssl/openssl',
};
/** The licence of each overlay port, which its vcpkg.json doesn't state. */
const PORT_LICENCES = { libtorrent: 'BSD-3-Clause', openssl: 'Apache-2.0' };

/**
 * The native libraries V2 links: deps.lock's, then the vcpkg overlay ports' (`{name, version}` from
 * each port's vcpkg.json), then Boost. A library in both (OpenSSL) is listed once and must have
 * the same version in both.
 */
export function nativeComponents(depsLock, ports) {
  const out = [...depsLock];
  for (const port of ports) {
    const same = out.find((c) => c.name === port.name);
    if (same) {
      if (same.version !== port.version) {
        throw new Error(`${port.name} is ${same.version} in deps.lock but ${port.version} in its vcpkg port`);
      }
      continue;
    }
    const licence = PORT_LICENCES[port.name];
    if (!licence) throw new Error(`the vcpkg port ${port.name} has no licence recorded in notices-lib.mjs`);
    out.push({ name: port.name, version: port.version, licence, source: PORT_SOURCES[port.name] });
  }
  return [...out, ...BASELINE_LIBRARIES];
}

/**
 * The third-party crates in the shipped binaries' graph: `cargo tree -e normal --target all
 * --prefix none -f '{p}'` lists them, and `cargo metadata` describes them. Workspace crates are V2's own.
 */
export function crateComponents(metadata, treeText) {
  const own = new Set(metadata.workspace_members);
  const wanted = new Set(
    treeText
      .split('\n')
      .map((l) => /^(\S+) v(\S+)/.exec(l.trim()))
      .filter(Boolean)
      .map((m) => `${m[1]}@${m[2]}`),
  );
  return metadata.packages
    .filter((p) => !own.has(p.id) && wanted.delete(`${p.name}@${p.version}`))
    .map((p) => ({
      name: p.name,
      version: p.version,
      licence: p.license ?? '',
      source: p.repository ?? `https://crates.io/crates/${p.name}/${p.version}`,
      authors: p.authors ?? [],
      dir: p.manifest_path.replace(/[\\/]Cargo\.toml$/, ''),
    }));
}

/** The npm production packages, from `pnpm licenses list --prod --json`. */
export function npmComponents(pnpmLicences) {
  return Object.values(pnpmLicences).flatMap((packages) =>
    packages.flatMap((p) =>
      p.versions.map((version, i) => ({
        name: p.name,
        version,
        licence: p.license ?? '',
        source: p.homepage ?? `https://www.npmjs.com/package/${p.name}/v/${version}`,
        authors: p.author ? [p.author] : [],
        dir: p.paths[i] ?? p.paths[0],
      })),
    ),
  );
}

/** A file holding a licence, a copyright or a notice, by its name. */
export const LICENCE_FILE = /^(licen[cs]e|copying|copyright|notice|unlicense)/i;
/** File names that name one licence, and the licence; the first match counts. */
const FILE_LICENCE = [
  [/apache/i, 'Apache-2.0'],
  [/(^|[-_.])mit([-_.]|$)/i, 'MIT'],
  [/0bsd/i, '0BSD'],
  [/bsd/i, 'BSD'],
  [/zlib/i, 'Zlib'],
  [/(^|[-_.])isc([-_.]|$)/i, 'ISC'],
  [/unicode/i, 'Unicode-3.0'],
  [/boost|bsl/i, 'BSL-1.0'],
  [/mpl/i, 'MPL-2.0'],
  [/^unlicense/i, 'Unlicense'],
  [/cc0/i, 'CC0-1.0'],
];

/**
 * Which of a component's licence files go into the notices: every file for a licence it is used
 * under, every file that names no single licence (LICENSE, COPYING, NOTICE…), and none for a
 * licence it isn't used under (LICENSE-APACHE when it is used under MIT). Machine-readable `.spdx`
 * files are left out.
 */
export function pickLicenceFiles(names, ids) {
  return names
    .filter((n) => LICENCE_FILE.test(n) && !/\.spdx$/i.test(n))
    .filter((n) => {
      const match = FILE_LICENCE.find(([re]) => re.test(n));
      if (!match) return true;
      const named = match[1];
      return ids.some((id) => id === named || (named === 'BSD' && id.startsWith('BSD-')));
    })
    .sort(byCodePoint);
}

/**
 * The file a licence file stands for, when it is a symbolic link that git checked out as a plain
 * file holding the link's target, as git does on Windows without `core.symlinks` (curl-sys's
 * `LICENSE` is `../LICENSE`). Only a single relative path to another licence file counts;
 * anything else is the text itself, and null is returned.
 */
export function linkTarget(text) {
  const t = text.trim();
  if (t.length > 255 || /[\n\r]/.test(t) || /^([a-z]:)?[\\/]/i.test(t)) return null;
  const name = t.split(/[\\/]/).pop();
  return LICENCE_FILE.test(name) && /[\\/]/.test(t) ? t : null;
}

/**
 * Where a stale THIRD_PARTY_NOTICES.md first differs from a fresh one: the line number and up to
 * `context` lines of each from there, or null when they are equal. For the check's message.
 */
export function firstDifference(committed, fresh, context = 6) {
  const a = committed.split('\n');
  const b = fresh.split('\n');
  const at = a.findIndex((line, i) => line !== b[i]);
  const line = at === -1 ? (a.length === b.length ? -1 : a.length) : at;
  if (line === -1) return null;
  return { line: line + 1, committed: a.slice(line, line + context), fresh: b.slice(line, line + context) };
}

/** Line endings and trailing spaces don't make two texts different. */
export function normaliseText(text) {
  return text
    .replace(/\r\n?/g, '\n')
    .split('\n')
    .map((l) => l.trimEnd())
    .join('\n')
    .trim();
}

function byCodePoint(a, b) {
  return a < b ? -1 : a > b ? 1 : 0;
}
const byComponent = (a, b) => byCodePoint(a.name, b.name) || byCodePoint(a.version, b.version);
const label = (c) => `${c.name} ${c.version}`;

/**
 * Checks every component and returns the problems: a licence that isn't allowed, or one that can't
 * be read. An empty list means the components may ship.
 * @param {{name: string, version: string, licence: string}[]} components
 */
export function licenceProblems(components) {
  return components
    .map((c) => {
      const choice = c.licence ? chooseLicence(c.licence, c) : { ok: false, reason: 'no licence declared' };
      return choice.ok ? null : `${label(c)} (${c.licence || 'none'}): ${choice.reason}`;
    })
    .filter(Boolean)
    .sort(byCodePoint);
}

/**
 * One platform's licence report (#17, from #76): every third-party component a release-candidate
 * build for `platform` (a Rust target triple) ships, with the licence V2 uses it under, and the
 * problems `licenceProblems` finds. The crates are those `cargo tree --target <platform>` lists.
 */
export function licenceReport({ platform, native, crates, npm }) {
  const entry = (kind) => (c) => {
    const choice = c.licence ? chooseLicence(c.licence, c) : { ok: false };
    return { kind, name: c.name, version: c.version, licence: c.licence, usedUnder: choice.ok ? choice.ids.join(' AND ') : null, source: c.source };
  };
  return {
    schema: 1,
    platform,
    components: [...native.map(entry('native')), ...[...crates].sort(byComponent).map(entry('crate')), ...[...npm].sort(byComponent).map(entry('npm'))],
    problems: licenceProblems([...native, ...crates, ...npm]),
  };
}

const cell = (s) => String(s).replaceAll('|', '\\|').replaceAll('\n', ' ');
const fence = (text) => `\`\`\`\`text\n${text}\n\`\`\`\``;

/**
 * THIRD_PARTY_NOTICES.md. Each component carries `texts`: the licence texts it ships, each
 * `{licence, text}`, or `standard` when it ships none and the SPDX text of its licence stands in.
 * @param {{native: object[], crates: object[], npm: object[], standardTexts: Record<string, string>}} input
 */
export function renderNotices({ native, crates, npm, standardTexts }) {
  const used = (c) => {
    const choice = chooseLicence(c.licence, c);
    return choice.ok ? choice.ids.join(' AND ') : 'NOT ALLOWED';
  };
  const table = (rows) =>
    [
      '| Component | Version | Licence | Used under | Source |',
      '|---|---|---|---|---|',
      ...[...rows].sort(byComponent).map((c) => `| ${cell(c.name)} | ${cell(c.version)} | ${cell(c.licence)} | ${cell(used(c))} | ${cell(c.source)} |`),
    ].join('\n');

  // Identical texts are printed once, with every component that ships them.
  const groups = new Map();
  const standard = new Map();
  for (const c of [...native, ...crates, ...npm].sort(byComponent)) {
    if (c.texts.length === 0) {
      for (const id of chooseLicence(c.licence, c).ids ?? []) {
        if (!standard.has(id)) standard.set(id, []);
        standard.get(id).push(c);
      }
      continue;
    }
    for (const t of c.texts) {
      const key = t.text.replace(/\s+/g, ' ').trim();
      if (!groups.has(key)) groups.set(key, { text: t.text, licences: new Set(), users: [] });
      const g = groups.get(key);
      g.licences.add(t.licence);
      if (!g.users.includes(label(c))) g.users.push(label(c));
    }
  }
  const textSections = [...groups.values()]
    .sort((a, b) => byCodePoint(a.users[0], b.users[0]))
    .map((g, i) => [`### ${i + 1}. ${[...g.licences].sort(byCodePoint).join(', ')}`, '', `Used by: ${g.users.join(', ')}.`, '', fence(g.text)].join('\n'));
  const standardSections = [...standard.entries()]
    .sort(([a], [b]) => byCodePoint(a, b))
    .map(([id, users]) => {
      const text = standardTexts[id];
      if (text === undefined) throw new Error(`no standard text for ${id} (scripts/notices/licenses/${id}.txt)`);
      const who = users.map((c) => {
        const authors = (c.authors ?? []).map((a) => a.replace(/\s*<[^>]*>/g, '').trim()).filter(Boolean);
        return `- ${label(c)}${authors.length ? `, by ${authors.join(', ')}` : ''}`;
      });
      return [`### ${id}, standard text`, '', 'These components ship no licence file. They are used under the standard text of their licence; each is named with the authors its package metadata gives:', '', ...who, '', fence(text)].join('\n');
    });

  return [
    '# Third-party notices',
    '',
    'Velocity Download Manager V2 contains the third-party components listed here. This file is generated by `pnpm notices` (`scripts/notices.mjs`) from the dependency graphs; don\'t edit it by hand. V2\'s own licence hasn\'t been chosen yet (#86).',
    '',
    `## Native libraries (${native.length})`,
    '',
    'Linked into V2\'s programs. The rules for each are in docs/LICENSING.md.',
    '',
    table(native),
    '',
    `## Rust crates (${crates.length})`,
    '',
    'In the app, the transfer engine, the torrent helper and the browser\'s native host, on every target platform.',
    '',
    table(crates),
    '',
    `## JavaScript packages (${npm.length})`,
    '',
    'In the app\'s user interface.',
    '',
    table(npm),
    '',
    '## Licence texts',
    '',
    ...[...textSections, ...standardSections].flatMap((s) => [s, '']),
  ].join('\n');
}
