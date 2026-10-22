// Pure logic for V2's native libraries (#35, docs/BUILDING_NATIVE.md): the lock file format, the
// download and signature checks, and the build identity record. Downloading, gpg and the builds
// themselves happen in fetch.mjs and build.mjs, so every rule here is unit tested.

/**
 * Licences a native library may have without a PO exception: the permissive class in
 * docs/DEPENDENCIES.md ("Allowed licences"), as SPDX identifiers.
 */
export const ALLOWED_LICENCES = new Set([
  'MIT',
  'Apache-2.0',
  'BSD-2-Clause',
  'BSD-3-Clause',
  'ISC',
  'Zlib',
  '0BSD',
  'Unlicense',
  'CC0-1.0',
  'BSL-1.0',
  'curl',
]);

const SHA256 = /^[0-9a-f]{64}$/;
const FINGERPRINT = /^[0-9A-F]{40}$/;
const VERSION = /^\d+(\.\d+)+$/;
const NAME = /^[a-z0-9][a-z0-9-]*$/;

/** `verified:<fingerprint>`, `unverifiable:<reason>` or `none-published`, or null when malformed. */
function parseSignature(token) {
  if (token === 'none-published') return { status: 'none-published' };
  const [status, value] = [token.slice(0, token.indexOf(':')), token.slice(token.indexOf(':') + 1)];
  if (status === 'verified' && FINGERPRINT.test(value)) return { status, fingerprint: value };
  if (status === 'unverifiable' && value.length > 0) return { status, reason: value };
  return null;
}

/**
 * Parses the native lock file: one library per line, `name version source-url sha256 signature
 * licence`, with `#` comments. Throws on the first malformed line, naming it, so a bad pin can
 * never reach a build.
 * @returns {{name: string, version: string, url: string, sha256: string,
 *   signature: {status: 'verified', fingerprint: string} | {status: 'unverifiable', reason: string} | {status: 'none-published'},
 *   licence: string}[]}
 */
export function parseLock(text) {
  const entries = [];
  text.split(/\r?\n/).forEach((raw, i) => {
    const line = raw.trim();
    if (!line || line.startsWith('#')) return;
    const at = `line ${i + 1}`;
    const cols = line.split(/\s+/);
    if (cols.length !== 6) throw new Error(`${at}: expected 6 columns (name version source-url sha256 signature licence), found ${cols.length}`);
    const [name, version, url, sha256, sig, licence] = cols;
    if (!NAME.test(name)) throw new Error(`${at}: invalid library name ${name}`);
    if (!VERSION.test(version)) throw new Error(`${at}: ${name} has an invalid version ${version}`);
    let parsed;
    try {
      parsed = new URL(url);
    } catch {
      parsed = null;
    }
    if (!parsed || parsed.protocol !== 'https:') throw new Error(`${at}: the source of ${name} must be an https URL`);
    const file = parsed.pathname.slice(parsed.pathname.lastIndexOf('/') + 1);
    if (!file.startsWith(`${name}-${version}.`)) throw new Error(`${at}: the source file ${file} must be ${name}-${version}.<archive>`);
    if (!SHA256.test(sha256)) throw new Error(`${at}: the SHA-256 of ${name} must be 64 lower-case hex digits`);
    const signature = parseSignature(sig);
    if (!signature) throw new Error(`${at}: the signature of ${name} must be verified:<40-hex fingerprint>, unverifiable:<reason> or none-published`);
    if (!ALLOWED_LICENCES.has(licence)) throw new Error(`${at}: ${name} has licence ${licence}, which is not in the allowed list (docs/DEPENDENCIES.md)`);
    if (entries.some((e) => e.name === name)) throw new Error(`${at}: ${name} is listed twice`);
    entries.push({ name, version, url, sha256, signature, licence });
  });
  if (!entries.length) throw new Error('the lock file lists no libraries');
  return entries;
}

const fileName = (entry) => entry.url.slice(entry.url.lastIndexOf('/') + 1);

/** Throws unless the downloaded archive's SHA-256 is the one the lock file pins. */
export function checkDownload(entry, actualSha256) {
  if (actualSha256 !== entry.sha256) {
    throw new Error(`${entry.name}: ${fileName(entry)} has SHA-256 ${actualSha256}, but the lock file pins ${entry.sha256}`);
  }
}

/** gpg status keywords that mean the signature must not be trusted (gnupg doc/DETAILS). */
const BAD_STATUS = ['BADSIG', 'ERRSIG', 'EXPSIG', 'EXPKEYSIG', 'REVKEYSIG', 'NO_PUBKEY'];

/**
 * The signature result to record for a library. For `verified`, `gpgStatus` is the output of
 * `gpg --status-fd 1 --verify` against a keyring holding only that library's pinned key; this
 * throws unless it shows exactly one valid signature whose primary key is the pinned fingerprint.
 * Other statuses need no gpg run and are recorded as the lock file states them.
 */
export function checkSignature(entry, gpgStatus) {
  const sig = entry.signature;
  if (sig.status === 'none-published') return 'none-published';
  if (sig.status === 'unverifiable') return `unverifiable:${sig.reason}`;

  const lines = (gpgStatus ?? '').split(/\r?\n/).filter((l) => l.startsWith('[GNUPG:] '));
  const keyword = (l) => l.split(' ')[1];
  const bad = lines.find((l) => BAD_STATUS.includes(keyword(l)));
  if (bad) throw new Error(`${entry.name}: signature check failed (${bad.slice(9)})`);
  const valid = lines.filter((l) => keyword(l) === 'VALIDSIG');
  if (valid.length !== 1) throw new Error(`${entry.name}: expected exactly one signature, gpg reported ${valid.length} valid`);
  const fields = valid[0].split(' ');
  const primary = fields[fields.length - 1];
  if (primary !== sig.fingerprint) throw new Error(`${entry.name}: signed by ${primary}, but the lock file expected ${sig.fingerprint}`);
  return `verified:${primary}`;
}

/** The libcurl version floor (research report section 8.1, ADR-0004). Each bump is its own reviewed change. */
export const LIBCURL_FLOOR = '8.23.0';
/** Protocols the engine needs. SFTP joins when its backend is decided and built (#52, #53). */
export const REQUIRED_PROTOCOLS = ['ftp', 'ftps', 'http', 'https'];
/** Protocols a build may contain. Anything else is disabled at build time, so no redirect or URL can reach it. */
export const ALLOWED_PROTOCOLS = [...REQUIRED_PROTOCOLS, 'sftp'];
/** libcurl features the engine needs: HTTP/2 through nghttp2, TLS, and compressed responses through zlib. */
export const REQUIRED_FEATURES = ['HTTP2', 'SSL', 'libz'];

/** Per target: the libraries to build, in build order, and the TLS backend libcurl must report. */
const TARGETS = {
  'x86_64-pc-windows-msvc': { libraries: ['zlib', 'nghttp2', 'curl'], tls: 'Schannel' },
  'x86_64-unknown-linux-gnu': { libraries: ['zlib', 'nghttp2', 'openssl', 'curl'], tls: 'OpenSSL' },
};

/** What the native build for a Rust target triple contains. macOS has no established build yet (#83). */
export function librariesFor(target) {
  const t = TARGETS[target];
  if (!t) throw new Error(`no native build is established for ${target} (established: ${Object.keys(TARGETS).join(', ')})`);
  return { libraries: [...t.libraries], tls: t.tls };
}

/** Reads the first line, `Protocols:` and `Features:` of `curl -V`, as printed by the built curl tool. */
export function parseCurlVersion(text) {
  const lines = (text ?? '').split(/\r?\n/);
  const head = /^curl \S+ .*?\blibcurl\/(\S+)\s+(.*)$/.exec(lines[0] ?? '');
  if (!head) throw new Error('not `curl -V` output: the first line must name curl and libcurl');
  const [tls, ...rest] = head[2].trim().split(/\s+/);
  const libraries = {};
  for (const token of rest) {
    const slash = token.indexOf('/');
    if (slash > 0) libraries[token.slice(0, slash)] = token.slice(slash + 1);
  }
  const list = (label) => (lines.find((l) => l.startsWith(`${label}:`)) ?? `${label}:`).slice(label.length + 1).trim().split(/\s+/).filter(Boolean);
  return { libcurl: head[1], tls, libraries, protocols: list('Protocols'), features: list('Features') };
}

function compareVersions(a, b) {
  const pa = a.split('.').map(Number);
  const pb = b.split('.').map(Number);
  for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
    const d = (pa[i] ?? 0) - (pb[i] ?? 0);
    if (d !== 0) return d;
  }
  return 0;
}

/**
 * Compares what a build actually linked (`parseCurlVersion` of its own curl tool) with the lock
 * file and the engine's requirements. Returns every problem found; an empty list means it matches.
 */
export function checkLinked(target, lock, linked) {
  const { libraries, tls } = librariesFor(target);
  const pinned = (name) => lock.find((e) => e.name === name);
  const problems = [];
  const curl = pinned('curl');
  if (!curl) problems.push('the lock file does not pin curl');
  else if (linked.libcurl !== curl.version) problems.push(`libcurl ${linked.libcurl} is linked, but the lock file pins curl ${curl.version}`);
  if (compareVersions(linked.libcurl, LIBCURL_FLOOR) < 0) problems.push(`libcurl ${linked.libcurl} is below the version floor ${LIBCURL_FLOOR}`);

  for (const name of libraries.filter((n) => n !== 'curl' && n !== 'openssl')) {
    const want = pinned(name)?.version;
    const got = linked.libraries[name];
    if (!want) problems.push(`the lock file does not pin ${name}`);
    else if (!got) problems.push(`${name} is not linked`);
    else if (got !== want) problems.push(`${name} ${got} is linked, but the lock file pins ${want}`);
  }

  const wantTls = tls === 'OpenSSL' ? `OpenSSL/${pinned('openssl')?.version}` : tls;
  if (linked.tls !== wantTls) problems.push(`TLS backend ${linked.tls} is linked, but the build needs ${wantTls}`);

  const missingProtocols = REQUIRED_PROTOCOLS.filter((p) => !linked.protocols.includes(p));
  if (missingProtocols.length) problems.push(`required protocols missing: ${missingProtocols.join(', ')}`);
  const missingFeatures = REQUIRED_FEATURES.filter((f) => !linked.features.includes(f));
  if (missingFeatures.length) problems.push(`required features missing: ${missingFeatures.join(', ')}`);
  const extra = linked.protocols.filter((p) => !ALLOWED_PROTOCOLS.includes(p));
  if (extra.length) problems.push(`protocols that should be disabled: ${extra.join(', ')}`);
  return problems;
}

/** The schema of build-info.json this code writes and reads. */
export const BUILD_INFO_SCHEMA = 1;

/**
 * Assembles build-info.json (docs/DEPENDENCIES.md, "Build identity record") for one native build.
 * Library sources come from the lock file, never from the build, so the record can't drift from it.
 */
export function createBuildInfo({ target, lock, signatures, options, linked, artifacts, toolchains, v2, cacheKey }) {
  const { libraries } = librariesFor(target);
  return {
    schema: BUILD_INFO_SCHEMA,
    target,
    v2,
    toolchains,
    cacheKey,
    libraries: libraries.map((name) => {
      const e = lock.find((l) => l.name === name);
      if (!e) throw new Error(`the lock file does not pin ${name}, which ${target} needs`);
      return { name, version: e.version, url: e.url, sha256: e.sha256, signature: signatures[name], licence: e.licence, options: options[name] };
    }),
    linked,
    artifacts,
  };
}

/**
 * Checks a build-info.json against the lock file: complete, from the pinned sources, and linking
 * what the lock file pins. A release is blocked while this returns any problem (DEPENDENCIES.md,
 * "Release blockers", items 3 to 5).
 */
export function checkBuildInfo(info, lock) {
  if (info?.schema !== BUILD_INFO_SCHEMA) return [`unknown build-info schema ${info?.schema} (this checker reads schema ${BUILD_INFO_SCHEMA})`];
  let needed;
  try {
    needed = librariesFor(info.target).libraries;
  } catch (e) {
    return [e.message];
  }
  const problems = [];
  if (!/^[0-9a-f]{40}$/.test(info.v2?.commit ?? '') || typeof info.v2?.dirty !== 'boolean') problems.push('the V2 commit is not recorded');
  if (!info.toolchains || !Object.keys(info.toolchains).length) problems.push('the toolchain versions are not recorded');
  if (!Array.isArray(info.artifacts) || !info.artifacts.length || info.artifacts.some((a) => !a.path || !SHA256.test(a.sha256 ?? ''))) {
    problems.push('the built artefacts and their SHA-256 are not recorded');
  }
  for (const name of needed) {
    const rec = info.libraries?.find((l) => l.name === name);
    const pin = lock.find((l) => l.name === name);
    if (!rec) {
      problems.push(`${name}: missing from the record`);
      continue;
    }
    if (!pin) {
      problems.push(`${name}: not in the lock file`);
      continue;
    }
    if (rec.version !== pin.version) problems.push(`${name}: the recorded version differs from the lock file`);
    if (rec.url !== pin.url) problems.push(`${name}: the recorded source URL differs from the lock file`);
    if (rec.sha256 !== pin.sha256) problems.push(`${name}: the recorded source SHA-256 differs from the lock file`);
    if (!rec.signature) problems.push(`${name}: no signature result recorded`);
    if (!Array.isArray(rec.options)) problems.push(`${name}: no build options recorded`);
  }
  if (!info.linked) problems.push('no `curl -V` result recorded');
  else problems.push(...checkLinked(info.target, lock, info.linked));
  return problems;
}

/**
 * Checks that every artefact build-info.json records still has its recorded SHA-256, so a library
 * replaced after the build is caught. `hashOf(path)` returns the SHA-256 of a path relative to the
 * build's prefix folder, or null when the file is missing.
 */
export function checkArtifacts(info, hashOf) {
  const problems = [];
  for (const a of info.artifacts ?? []) {
    const parts = String(a.path).split(/[\\/]/);
    if (!a.path || a.path.startsWith('/') || /^[A-Za-z]:/.test(a.path) || parts.includes('..')) {
      problems.push(`${a.path} is not a path inside the build folder`);
      continue;
    }
    const actual = hashOf(a.path);
    if (actual === null || actual === undefined) problems.push(`${a.path} is missing`);
    else if (actual !== a.sha256) problems.push(`${a.path} changed after the build: its SHA-256 differs from build-info.json`);
  }
  return problems;
}
