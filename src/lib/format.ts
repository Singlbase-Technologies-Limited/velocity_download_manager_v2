const UNITS = ['B', 'KiB', 'MiB', 'GiB', 'TiB'] as const;

/** A size that is known: not `null`, a number, and not negative. */
function known(bytes: number | null | undefined): bytes is number {
  return bytes !== null && bytes !== undefined && Number.isFinite(bytes) && bytes >= 0;
}

/** The unit a size is said in: the largest it is at least one of. */
function unitOf(bytes: number): number {
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < UNITS.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return unit;
}

/** Bytes in binary units with one decimal, as the tray says them (src-tauri/src/tray.rs): "1.5 MiB". "—" when unknown. */
export function formatBytes(bytes: number | null | undefined): string {
  if (!known(bytes)) return '—';
  const unit = unitOf(bytes);
  return unit === 0 ? `${String(bytes)} B` : `${(bytes / 1024 ** unit).toFixed(1)} ${UNITS[unit] ?? ''}`;
}

export function formatSpeed(bytesPerSecond: number | null | undefined): string {
  return known(bytesPerSecond) ? `${formatBytes(bytesPerSecond)}/s` : '—';
}

/** `bytes` in `unit`, with two decimals from GiB up, so a large download visibly moves every ~10 MiB (V1). */
function inUnit(bytes: number, unit: number): string {
  const digits = unit >= 3 ? 2 : unit >= 1 ? 1 : 0;
  return `${(bytes / 1024 ** unit).toFixed(digits)} ${UNITS[unit] ?? ''}`;
}

/** A transferred amount, more precise than {@link formatBytes} for large values. */
export function formatTransferredBytes(bytes: number | null | undefined): string {
  return known(bytes) ? inUnit(bytes, unitOf(bytes)) : '—';
}

/** "0.50 GiB of 3.80 GiB": both sides in the total's unit, so they compare at a glance. */
export function formatTransferred(completed: number, total: number | null | undefined): string {
  if (!known(total) || total === 0) return `${formatTransferredBytes(completed)} of unknown size`;
  const unit = unitOf(total);
  return `${inUnit(completed, unit)} of ${inUnit(total, unit)}`;
}

/** "59s", "1m 01s", "3h 02m", "2d 2h"; "—" when unknown. */
export function formatDuration(seconds: number | null | undefined): string {
  if (!known(seconds)) return '—';
  const s = Math.round(seconds);
  if (s < 60) return `${String(s)}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${String(m)}m ${String(s % 60).padStart(2, '0')}s`;
  const h = Math.floor(m / 60);
  if (h < 48) return `${String(h)}h ${String(m % 60).padStart(2, '0')}m`;
  return `${String(Math.floor(h / 24))}d ${String(h % 24)}h`;
}

/** One decimal below 10%, none above; "—" when unknown. */
export function formatPercent(p: number | null | undefined): string {
  if (p === null || p === undefined || !Number.isFinite(p)) return '—';
  if (p >= 100) return '100%';
  return `${p.toFixed(p < 10 ? 1 : 0)}%`;
}

const timeFmt = new Intl.DateTimeFormat(undefined, { timeStyle: 'short' });
const monthDayTimeFmt = new Intl.DateTimeFormat(undefined, { month: 'short', day: 'numeric', hour: 'numeric', minute: '2-digit' });
const monthDayYearFmt = new Intl.DateTimeFormat(undefined, { dateStyle: 'medium' });

/** A date for a table cell (V1): the time only today, no year within this year. `ms` since the Unix epoch. */
export function formatShortDate(ms: number | null, now = new Date()): string {
  if (ms === null) return '—';
  const d = new Date(ms);
  if (d.toDateString() === now.toDateString()) return timeFmt.format(d);
  if (d.getFullYear() === now.getFullYear()) return monthDayTimeFmt.format(d);
  return monthDayYearFmt.format(d);
}

/** "in 1m 00s" or "30s ago", from `ms` since the Unix epoch. */
export function relativeFromNow(ms: number, now = Date.now()): string {
  const diff = Math.round((ms - now) / 1000);
  return diff > 0 ? `in ${formatDuration(diff)}` : `${formatDuration(-diff)} ago`;
}
