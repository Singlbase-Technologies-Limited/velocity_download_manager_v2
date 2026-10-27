// The name a new download is saved under, for the Add form's preview (#31). Mirrors
// `name_for_download` in crates/velocity-core/src/naming.rs; `naming.test.ts` checks it against
// cases generated from Rust. The native side decides: this only previews.

/** What a download would be named: `null` when the server names it, or not allowed. */
export type NamePreview = { ok: true; name: string | null } | { ok: false };

/** `MAX_FILE_NAME_BYTES`: the longest name, in UTF-8 bytes. */
const MAX_FILE_NAME_BYTES = 240;
/** `FALLBACK_FILE_NAME`. */
const FALLBACK_FILE_NAME = 'download';
/** `PART_FILE_SUFFIX`: no download is ever named like the engine's part files. */
const PART_FILE_SUFFIX = '.vdmpart';
/** `MAX_KEPT_EXTENSION_BYTES`. */
const MAX_KEPT_EXTENSION_BYTES = 15;
/** `WINDOWS_RESERVED`: device names Windows reserves, with or without an extension. */
const WINDOWS_RESERVED = new Set([
  'CON', 'PRN', 'AUX', 'NUL', 'COM1', 'COM2', 'COM3', 'COM4', 'COM5', 'COM6', 'COM7', 'COM8', 'COM9',
  'COM¹', 'COM²', 'COM³', 'LPT1', 'LPT2', 'LPT3', 'LPT4', 'LPT5', 'LPT6', 'LPT7', 'LPT8', 'LPT9',
  'LPT¹', 'LPT²', 'LPT³', 'CONIN$', 'CONOUT$',
]);
/** `SCRIPT_EXTENSIONS`: such a link names the program that serves the file, not the file. */
const SCRIPT_EXTENSIONS = new Set(['php', 'asp', 'aspx', 'jsp', 'cgi']);

const FORBIDDEN = /[\p{Cc}/\\<>:"|?*]/gu;
const encoder = new TextEncoder();
const bytes = (s: string): number => encoder.encode(s).length;

/** Rust's `trim`, `trim_start`, `trim_end`: Unicode `White_Space`, which JavaScript's differ from. */
const trim = (s: string): string => s.replace(/^\p{White_Space}+|\p{White_Space}+$/gu, '');
const trimEnd = (s: string): string => s.replace(/\p{White_Space}+$/u, '');
/** Rust's `to_ascii_lowercase` and `to_ascii_uppercase`: other letters stay as they are. */
const asciiLower = (s: string): string => s.replace(/[A-Z]/g, (c) => c.toLowerCase());
const asciiUpper = (s: string): string => s.replace(/[a-z]/g, (c) => c.toUpperCase());

/** `rsplit_once('.')`. */
function splitLastDot(name: string): [string, string] | null {
  const i = name.lastIndexOf('.');
  return i < 0 ? null : [name.slice(0, i), name.slice(i + 1)];
}

/** `name` with `tag` before its extension, cut to `MAX_FILE_NAME_BYTES` at a character boundary. */
function shorten(name: string, tag: string): string {
  const split = splitLastDot(name);
  const [stem, ext] =
    split && split[0] !== '' && bytes(split[1]) <= MAX_KEPT_EXTENSION_BYTES ? [split[0], `.${split[1]}`] : [name, ''];
  const room = Math.max(0, MAX_FILE_NAME_BYTES - bytes(tag) - bytes(ext));
  if (bytes(stem) <= room) return `${stem}${tag}${ext}`;
  let cut = '';
  for (const c of stem) {
    if (bytes(cut + c) > room) break;
    cut += c;
  }
  return `${cut.replace(/[. ]+$/, '')}${tag}${ext}`;
}

/** `sanitize_file_name`: one plain path component on every supported platform. */
function sanitizeFileName(name: string): string {
  const mapped = name.replace(FORBIDDEN, '_');
  let out = trim(trim(mapped).replace(/^\.+/, '')).replace(/[. ]+$/, '');
  if (out === '') return FALLBACK_FILE_NAME;
  if (WINDOWS_RESERVED.has(asciiUpper(trimEnd(out.split('.')[0] ?? '')))) out = `_${out}`;
  if (asciiLower(out).endsWith(PART_FILE_SUFFIX)) out += '_';
  return shorten(out, '');
}

/** `check_file_name`: the name is already a safe file name. */
function isSafeFileName(name: string): boolean {
  return name !== '' && sanitizeFileName(name) === name;
}

/** `extension_of`: the lowercase extension, `zip` for `a.ZIP`. */
function extensionOf(name: string): string | null {
  const split = splitLastDot(name);
  if (!split || split[0] === '' || split[1] === '' || split[1].includes(' ')) return null;
  return asciiLower(split[1]);
}

/** `percent_decode`: `%XX` escapes as UTF-8; bytes that aren't become U+FFFD. */
function percentDecode(s: string): string {
  const raw = encoder.encode(s);
  const out: number[] = [];
  const hex = (b: number | undefined): number | null => {
    const c = b === undefined ? '' : String.fromCharCode(b);
    return /^[0-9a-fA-F]$/.test(c) ? parseInt(c, 16) : null;
  };
  for (let i = 0; i < raw.length; i++) {
    const high = raw[i] === 0x25 && i + 2 < raw.length ? hex(raw[i + 1]) : null;
    const low = high === null ? null : hex(raw[i + 2]);
    if (high !== null && low !== null) {
      out.push(high * 16 + low);
      i += 2;
    } else {
      out.push(raw[i] as number);
    }
  }
  return new TextDecoder('utf-8').decode(new Uint8Array(out));
}

/** `file_name_from_url`: the decoded, sanitised last path segment. */
function fileNameFromUrl(url: string): string | null {
  const at = url.indexOf('://');
  if (at < 0) return null;
  const rest = url.slice(at + 3);
  const pathStart = rest.search(/[/?#]/);
  if (pathStart < 0 || rest[pathStart] !== '/') return null;
  const path = rest.slice(pathStart).split(/[?#]/, 1)[0] ?? '';
  const segment = path.split('/').filter((s) => s !== '').pop();
  if (segment === undefined) return null;
  const decoded = percentDecode(segment);
  return trim(decoded) === '' ? null : sanitizeFileName(decoded);
}

/** `file_name_for_link`: the link's name, only when it has an extension that isn't a script's. */
function fileNameForLink(url: string): string | null {
  const name = fileNameFromUrl(url);
  const ext = name === null ? null : extensionOf(name);
  return ext !== null && !SCRIPT_EXTENSIONS.has(ext) ? name : null;
}

/** `with_link_extension`: a typed name keeps the link's file type, unless it has a known one. */
function withLinkExtension(typed: string, linkName: string | null, known: ReadonlySet<string>): string | null {
  if (!isSafeFileName(typed)) return null;
  const ext = linkName === null ? null : extensionOf(linkName);
  if (ext === null) return typed;
  const own = extensionOf(typed);
  if (own !== null && (own === ext || known.has(own))) return typed;
  const name = `${typed}.${ext}`;
  return isSafeFileName(name) ? name : null;
}

/**
 * The name a new download of `url` is saved under: the typed one, trimmed and keeping the link's
 * file type, else the link's. `known` are the extensions the categories list.
 */
export function nameForDownload(url: string, typed: string | null, known: ReadonlySet<string>): NamePreview {
  const linkName = fileNameForLink(url);
  const name = typed === null ? '' : trim(typed);
  if (name === '') return { ok: true, name: linkName };
  const withExtension = withLinkExtension(name, linkName, known);
  return withExtension === null ? { ok: false } : { ok: true, name: withExtension };
}
