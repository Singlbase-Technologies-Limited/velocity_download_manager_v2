// Pure logic behind the field test (#262, tests/field/README.md):
// - hostOf, hideLinks: a link is never written whole, only its scheme and host (PO decision on
//   #262: the PO's own links stay out of every report).
// - sample: one download as the field test watches it, from the app's snapshot and its files.
// - findings: what looks wrong between two samples of the same download.
// No I/O here; session.mjs supplies the inputs.

/** How long a download may show Downloading without a new byte before it is flagged. */
export const QUIET_MS = 30_000;

/** The host of a link, or null for one without a host (a magnet link, a file path). */
export function hostOf(link) {
  try {
    const host = new URL(link).hostname;
    return host === '' ? null : host;
  } catch {
    return null;
  }
}

const LINK = /\b([a-z][a-z0-9+.-]*):\/\/([^\s/?#"'<>]*)[^\s"'<>]*/gi;
const TRAILING = /[.,:;)\]]+$/;

/** `text` with every link cut to `scheme://host/…`: no login, port, path or query. */
export function hideLinks(text) {
  return text.replace(LINK, (whole, scheme, authority) => {
    const tail = TRAILING.exec(whole)?.[0] ?? '';
    const host = authority.slice(authority.lastIndexOf('@') + 1).replace(/:\d+$/, '');
    return `${scheme}://${host}/…${tail}`;
  });
}

function why(error) {
  if (!error) return null;
  const status = error.httpStatus == null ? '' : `, HTTP ${error.httpStatus}`;
  return `${hideLinks(error.message)} [${error.class}${status}]`;
}

/**
 * One download as the field test watches it.
 * @param {object} record a `DownloadRecord` from the app's `get_snapshot`
 * @param {{partBytes?: number|null, fileBytes?: number|null}} files the part file's and the file's length on disk
 */
export function sample(record, files) {
  const p = record.progress;
  return {
    id: record.id,
    name: record.fileName,
    host: hostOf(record.sourceUrl),
    status: record.status,
    done: p.completedBytes,
    total: p.totalBytes,
    speed: p.downloadSpeedBps,
    connections: p.connections,
    allowed: p.connectionDetail?.allowed ?? null,
    requested: p.connectionDetail?.requested ?? null,
    limitedBy: p.connectionDetail?.limitedBy ?? null,
    error: why(record.error),
    retryAtMs: record.retryAtMs,
    partBytes: files.partBytes ?? null,
    fileBytes: files.fileBytes ?? null,
  };
}

const size = (bytes) => (bytes == null ? 'unknown' : `${bytes} bytes`);

/**
 * What looks wrong from `before` to `now`, two samples of one download.
 * @param {number} quietMs how long its bytes have stayed the same, up to `now`
 * @returns {string[]}
 */
export function findings(before, now, quietMs) {
  const out = [];
  const id = now.id;
  // A user's cancel or retry queues it again first; anything else that loses bytes is a finding.
  if (now.done < before.done && now.status !== 'queued') {
    out.push(`${id}: progress went backwards, from ${before.done} to ${now.done} bytes`);
  }
  if (before.total != null && now.total !== before.total) {
    out.push(`${id}: the size changed, from ${size(before.total)} to ${size(now.total)}`);
  }
  if (before.status === 'completed' && now.status !== 'completed') {
    out.push(`${id}: Completed, then ${now.status}`);
  }
  if (now.status === 'downloading' && now.done === before.done && quietMs >= QUIET_MS) {
    out.push(`${id}: no new bytes for ${Math.floor(quietMs / 1000)} s, but it still shows downloading`);
  }
  return out;
}
