// The Add dialog's input rules (#60), ported from V1's addLogic.ts. They flag mistakes as the user
// types; the native side checks everything again (`Manager::add_request`) and decides.
import { availability } from '../../capabilities';
import type { Capabilities, Checksum, DownloadKind, HashAlgorithm, Header } from '../../contract';

/** The most links and `.torrent` files one request adds (`MAX_ADD`). */
export const MAX_ADD = 500;

/** Pasted text as links: one per line or space, each once (V1's `splitSources`). */
export function splitLinks(text: string): string[] {
  return [...new Set(text.split(/\s+/).filter((s) => s !== ''))];
}

const SCHEMES: Record<string, DownloadKind> = { 'http:': 'http', 'https:': 'http', 'ftp:': 'ftp', 'ftps:': 'ftp', 'sftp:': 'sftp' };

const isMagnet = (link: string) => link.slice(0, 7).toLowerCase() === 'magnet:';

/**
 * Whether `link` is to a `.torrent` file, which Velocity fetches and adds as a torrent: an http or
 * https link whose path ends in `.torrent`. Mirrors `is_torrent_link` in Rust.
 */
export function isTorrentLink(link: string): boolean {
  try {
    const u = new URL(link.trim());
    return (u.protocol === 'http:' || u.protocol === 'https:') && u.pathname.toLowerCase().endsWith('.torrent');
  } catch {
    return false;
  }
}

/** What a link downloads with: a direct protocol, a magnet link, or a `.torrent` link; `null` for none. */
export function linkKind(link: string): 'http' | 'ftp' | 'sftp' | 'magnet' | 'torrentLink' | null {
  if (isMagnet(link)) return 'magnet';
  if (isTorrentLink(link)) return 'torrentLink';
  try {
    const kind = SCHEMES[new URL(link).protocol.toLowerCase()];
    return kind === 'http' || kind === 'ftp' || kind === 'sftp' ? kind : null;
  } catch {
    return null;
  }
}

/**
 * Why `link` can't be added, in words for the user, or `null`. `capabilities` are the transfer
 * engine's: a protocol it lacks is refused with the reason, before anything is sent.
 */
export function linkProblem(link: string, capabilities: Capabilities | undefined): string | null {
  if (isMagnet(link)) return null;
  let u: URL;
  try {
    u = new URL(link);
  } catch {
    return 'This is not a complete link. It should start with https://, http://, ftp:// or sftp://.';
  }
  const kind = SCHEMES[u.protocol.toLowerCase()];
  if (kind === undefined) return 'Only http, https, ftp, ftps, sftp and magnet links are supported.';
  if (u.hostname === '') return 'This is not a complete link. It should start with https://, http://, ftp:// or sftp://.';
  if (u.username !== '' || u.password !== '') return 'Put the user name and password in their own fields, not in the link.';
  if (capabilities && !isTorrentLink(link)) {
    const available = availability(capabilities, kind);
    if (!available.available) return available.reason;
  }
  return null;
}

/** "Name: value" lines as headers, or the first malformed line (V1's `parseHeaders`). */
export function parseHeaders(text: string): { headers: Header[]; error: string | null } {
  const headers: Header[] = [];
  const lines = text
    .split('\n')
    .map((l) => l.trim())
    .filter(Boolean);
  for (const [i, line] of lines.entries()) {
    const at = line.indexOf(':');
    if (at <= 0) return { headers, error: `Line ${String(i + 1)} must look like "Name: value".` };
    headers.push({ name: line.slice(0, at).trim(), value: line.slice(at + 1).trim() });
  }
  return { headers, error: null };
}

const ALGORITHMS: Record<string, { algorithm: HashAlgorithm; hexLength: number }> = {
  md5: { algorithm: 'md5', hexLength: 32 },
  sha1: { algorithm: 'sha1', hexLength: 40 },
  sha224: { algorithm: 'sha224', hexLength: 56 },
  sha256: { algorithm: 'sha256', hexLength: 64 },
  sha384: { algorithm: 'sha384', hexLength: 96 },
  sha512: { algorithm: 'sha512', hexLength: 128 },
};

/** A checksum written as V1 took it, `sha-256=<hex>` (or `sha256:<hex>`); empty for none. */
export function parseChecksum(text: string): { checksum: Checksum | null; error: string | null } {
  const trimmed = text.trim();
  if (trimmed === '') return { checksum: null, error: null };
  const match = /^([a-z0-9-]+)\s*[=:]\s*([^\s]+)$/i.exec(trimmed);
  if (!match) return { checksum: null, error: 'Write the checksum like “sha-256=…”, with its algorithm first.' };
  const known = ALGORITHMS[(match[1] ?? '').toLowerCase().replace('-', '')];
  if (!known) return { checksum: null, error: 'Use md5, sha-1, sha-224, sha-256, sha-384 or sha-512.' };
  const hex = (match[2] ?? '').toLowerCase();
  if (hex.length !== known.hexLength || !/^[0-9a-f]+$/.test(hex)) {
    return { checksum: null, error: `This checksum must be ${String(known.hexLength)} hexadecimal characters.` };
  }
  return { checksum: { algorithm: known.algorithm, hex }, error: null };
}
