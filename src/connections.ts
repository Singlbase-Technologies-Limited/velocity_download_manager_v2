// How many connections a download uses, and why not more (#57, V1 #44's semantics): from the
// engine's `ConnectionDetail` (#47). The download details (R8) show it (`ConnectionLine.tsx`).
import type { ConnectionDetail, ConnectionLimit } from './contract';

/** Why a download uses fewer connections than it asked for, in words for the user. */
export function limitReason(limit: ConnectionLimit): string {
  switch (limit) {
    case 'perHostCap':
      return 'the performance profile allows no more to one server';
    case 'sharedBudget':
      return 'other downloads are sharing the connections';
    case 'serverThrottling':
      return 'the server asked Velocity to slow down';
    case 'rampingUp':
      return 'Velocity is still finding how many connections help';
    case 'noGain':
      return 'more connections stopped making it faster';
    case 'fileSize':
      return 'the rest of the file is too small to split further';
  }
}

/** "Using 3 connections, up to 4 of the 8 asked for: …". */
export function connectionSummary(active: number, detail: ConnectionDetail | null): string {
  if (!detail && active === 0) return 'Not connected.';
  const using = `Using ${String(active)} ${active === 1 ? 'connection' : 'connections'}`;
  if (!detail?.limitedBy) return `${using}.`;
  const { allowed, requested, limitedBy } = detail;
  return `${using}, up to ${String(allowed)} of the ${String(requested)} asked for: ${limitReason(limitedBy)}.`;
}
