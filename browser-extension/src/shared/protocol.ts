/**
 * Native messaging protocol v1, unchanged from V1 and mirrored by the native host (#65).
 * The extension can only ping, ask for status, or offer a download link. It never sends
 * paths, commands or engine options.
 */
export const PROTOCOL_VERSION = 1;

export type RequestSource = 'intercept' | 'contextMenu' | 'popup';

export interface AddPayload {
  url: string;
  referrer?: string;
  filename?: string;
  fileSize?: number;
  mime?: string;
  source: RequestSource;
}

export type Request =
  | { v: 1; id: string; type: 'ping' }
  | { v: 1; id: string; type: 'app.status' }
  | { v: 1; id: string; type: 'download.add'; payload: AddPayload };

export interface ResponseError {
  code: string;
  message: string;
}

export type Response = { v: number; id: string; ok: true; result: Record<string, unknown> } | { v: number; id: string; ok: false; error: ResponseError };

const MAX_URL = 8192;

export function newId(): string {
  return crypto.randomUUID();
}

function isHttpUrl(s: string): boolean {
  try {
    const u = new URL(s);
    return u.protocol === 'http:' || u.protocol === 'https:';
  } catch {
    return false;
  }
}

/** Build a download.add request, dropping optional fields the desktop app would reject. */
export function buildAdd(p: AddPayload): Request {
  const payload: AddPayload = { url: p.url, source: p.source };
  if (p.referrer && p.referrer.length <= MAX_URL && isHttpUrl(p.referrer)) payload.referrer = p.referrer;
  if (p.filename && p.filename.length <= 1024) payload.filename = p.filename.split(/[\\/]/).pop() ?? p.filename;
  if (typeof p.fileSize === 'number' && Number.isSafeInteger(p.fileSize) && p.fileSize > 0) payload.fileSize = p.fileSize;
  if (p.mime && p.mime.length <= 255 && !/\p{Cc}/u.test(p.mime)) payload.mime = p.mime;
  return { v: PROTOCOL_VERSION, id: newId(), type: 'download.add', payload };
}

/** A magnet link carrying an exact topic (`xt=`). The desktop app validates the rest. */
export function isMagnet(url: string): boolean {
  return /^magnet:\?/i.test(url) && /[?&]xt=/i.test(url);
}

export function canSend(url: string): boolean {
  return url.length <= MAX_URL && (isHttpUrl(url) || isMagnet(url));
}

/** Links the browser itself can download when Velocity cannot take them. */
export function browserCanDownload(url: string): boolean {
  return url.length <= MAX_URL && isHttpUrl(url);
}

/** Validate a response from the native host. Anything unexpected becomes an error response. */
export function parseResponse(raw: unknown, expectedId: string): Response {
  const bad = (message: string): Response => ({ v: PROTOCOL_VERSION, id: expectedId, ok: false, error: { code: 'INVALID_RESPONSE', message } });
  if (typeof raw !== 'object' || raw === null) return bad('The desktop application sent an invalid response.');
  const r = raw as Record<string, unknown>;
  if (r.v !== PROTOCOL_VERSION) return bad('The desktop application uses a different protocol version. Update both to the same version.');
  if (r.ok === true && typeof r.result === 'object' && r.result !== null) return { v: 1, id: String(r.id), ok: true, result: r.result as Record<string, unknown> };
  if (r.ok === false && typeof r.error === 'object' && r.error !== null) {
    const e = r.error as Record<string, unknown>;
    if (typeof e.code === 'string' && typeof e.message === 'string') return { v: 1, id: String(r.id), ok: false, error: { code: e.code, message: e.message } };
  }
  return bad('The desktop application sent an invalid response.');
}
