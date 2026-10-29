// What one Add request did, sorted for the user (#60): added, taken names, duplicates and failures.
import type { AddRequest, AddResult, DestinationCheck, Duplicate } from '../../contract';
import { ERROR_MESSAGES } from '../../lib/errors';
import { linkKind } from './addLogic';

/** What a request did, sorted for the user (#60). */
export interface Outcome {
  /** Added so far, this request and the ones before it. */
  added: number;
  conflicts: { source: string; check: DestinationCheck; torrent: boolean }[];
  duplicates: { source: string; duplicate: Duplicate }[];
  errors: { source: string; message: string }[];
  /** What was sent: which sources are links and which `.torrent` files. */
  request: AddRequest;
}

export const plural = (n: number, word: string) => `${String(n)} ${word}${n === 1 ? '' : 's'}`;

/** Sorts each link's result into what was added, and what needs the user. */
export function sortOutcome(results: AddResult[], request: AddRequest, addedBefore: number): Outcome {
  const outcome: Outcome = { added: addedBefore, conflicts: [], duplicates: [], errors: [], request };
  for (const { source, outcome: o } of results) {
    switch (o.kind) {
      case 'added':
        outcome.added += 1;
        break;
      case 'conflict': {
        const kind = linkKind(source);
        const torrent = request.torrentFiles.includes(source) || kind === 'magnet' || kind === 'torrentLink';
        outcome.conflicts.push({ source, check: o.check, torrent });
        break;
      }
      case 'duplicate':
        outcome.duplicates.push({ source, duplicate: o.duplicate });
        break;
      case 'failed':
        outcome.errors.push({ source, message: ERROR_MESSAGES[o.code] });
        break;
      case 'notFetched':
        outcome.errors.push({ source, message: o.error.message });
        break;
    }
  }
  return outcome;
}

/** `sources` as a request's links and `.torrent` files, as they were in `request`. */
export function only(sources: string[], request: AddRequest): Pick<AddRequest, 'links' | 'torrentFiles'> {
  return {
    links: request.links.filter((l) => sources.includes(l)),
    torrentFiles: request.torrentFiles.filter((f) => sources.includes(f)),
  };
}
