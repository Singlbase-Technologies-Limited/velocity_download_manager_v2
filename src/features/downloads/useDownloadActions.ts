import { useMemo, useState } from 'react';
import { toast } from 'sonner';
import { confirmAction } from '../../components/confirmStore';
import type { BatchResult, DownloadRecord, ErrorCode, KeptFile, KeptReason, MoveDirection } from '../../contract';
import { displayName } from '../../downloads/list';
import { api } from '../../lib/api';
import { ApiError, ERROR_MESSAGES, toApiError } from '../../lib/errors';
import { useDownloadsStore } from '../../stores/downloads';
import { useUiStore } from '../../stores/ui';

const plural = (n: number, word: string) => `${String(n)} ${word}${n === 1 ? '' : 's'}`;

const KEPT: Record<KeptReason, string> = {
  notPlain: 'it isn’t a plain file',
  tooLarge: 'it is too large to check',
  behindLink: 'it is behind a link',
  notEmpty: 'the folder isn’t empty',
  failed: 'it couldn’t be deleted',
};

/** The files left in place, with why the first was. */
function reportKept(kept: readonly KeptFile[]) {
  const first = kept[0];
  if (first) toast.warning(`${plural(kept.length, 'file')} left in place, as ${KEPT[first.reason]}: ${first.path}`);
}

/** What a command over several downloads did (V1's report): every one, or how many and why the rest couldn't. */
function report(r: BatchResult, verb: string) {
  const first = r.failed[0];
  if (!first) toast.success(r.done.length > 1 ? `${verb} ${plural(r.done.length, 'download')}` : verb);
  else if (r.done.length > 0) toast.warning(`${verb} ${String(r.done.length)}; ${String(r.failed.length)} couldn’t be changed: ${ERROR_MESSAGES[first.code]}`);
  else toast.error(ERROR_MESSAGES[first.code]);
  reportKept(r.kept);
}

/** Why a file or folder couldn't be opened, in the words for opening (#61); other codes keep their usual sentence. */
const OPEN_FAILED: Partial<Record<ErrorCode, string>> = {
  NOT_FOUND: 'The file is no longer there. It may have been moved or deleted.',
  INVALID_PATH: 'Velocity doesn’t open a file reached through a link.',
  INVALID_STATE: 'The download hasn’t finished, so there is no file to open yet.',
  DESTINATION_MISSING: 'The download’s folder is no longer there.',
  IO: 'The system couldn’t start the program that opens it.',
};

async function copy(text: string, what: string) {
  try {
    await navigator.clipboard.writeText(text);
    toast.success(`${what} copied`);
  } catch {
    toast.error(`Couldn’t copy the ${what.toLowerCase()}`);
  }
}

/**
 * Everything the user does to downloads (V1's useDownloadActions): confirmations before destructive ones, a toast
 * for each result, and `busy` while a command is being sent, so it can't be sent twice. What the downloads become
 * comes from the next snapshot, never from here.
 */
export function useDownloadActions() {
  const [pending, setPending] = useState(0);
  const askLogin = useUiStore((s) => s.askLogin);
  const actions = useMemo(() => {
    async function send(fn: () => Promise<unknown>, messages: Partial<Record<ErrorCode, string>> = {}) {
      setPending((n) => n + 1);
      try {
        await fn();
      } catch (e) {
        const error = toApiError(e);
        toast.error(messages[error.code] ?? error.message);
      } finally {
        setPending((n) => n - 1);
      }
    }
    /** Downloads that came back without their login are asked for it first (PO decision on #59). */
    function withLogin(ids: string[], then: 'resume' | 'retry', fn: (ids: string[]) => Promise<BatchResult>, verb: string) {
      const waiting = new Set(useDownloadsStore.getState().snapshot?.needsLogin);
      const ready = ids.filter((id) => !waiting.has(id));
      const first = ids.find((id) => waiting.has(id));
      if (first) askLogin({ id: first, then });
      return ready.length > 0 ? send(async () => { report(await fn(ready), verb); }) : Promise.resolve();
    }
    return {
      pause: (ids: string[]) => send(async () => { report(await api.pauseDownloads(ids), 'Paused'); }),
      resume: (ids: string[]) => withLogin(ids, 'resume', api.resumeDownloads, 'Resumed'),
      retry: (ids: string[]) => withLogin(ids, 'retry', api.retryDownloads, 'Retrying'),
      cancel: (ids: string[]) => send(async () => { report(await api.cancelDownloads(ids), 'Cancelled'); }),
      async remove(items: DownloadRecord[]) {
        const active = items.filter((d) => !['completed', 'failed', 'removed', 'unknown'].includes(d.status));
        if (active.length > 0) {
          const ok = await confirmAction({
            title: `Remove ${plural(items.length, 'download')}?`,
            description: `${plural(active.length, 'download')} still in progress will be stopped. Files already on disk are kept.`,
            confirmLabel: 'Stop and remove',
            destructive: true,
          });
          if (!ok) return;
        }
        await send(async () => { report(await api.removeDownloads(items.map((d) => d.id), false), 'Removed from list'); });
      },
      async removeAndDelete(items: DownloadRecord[]) {
        const finished = items.some((d) => d.status === 'completed' || d.status === 'seeding');
        const ok = await confirmAction({
          title: `Delete files for ${plural(items.length, 'download')}?`,
          description: `This permanently deletes ${finished ? 'the downloaded files' : 'the partly downloaded files'} from disk and removes the downloads from the list. This cannot be undone.`,
          confirmLabel: 'Delete files',
          destructive: true,
        });
        if (!ok) return;
        await send(async () => { report(await api.removeDownloads(items.map((d) => d.id), true), 'Deleted'); });
      },
      move: (id: string, direction: MoveDirection) => send(() => api.moveDownload(id, direction)),
      stopSeeding: (ids: string[]) =>
        send(async () => {
          await Promise.all(ids.map((id) => api.stopSeeding(id)));
          toast.success(ids.length > 1 ? `Stopped seeding ${plural(ids.length, 'torrent')}` : 'Stopped seeding');
        }),
      /** Opens the finished file; a program only once the user confirmed it (PO decision on #61). */
      openFile: (d: DownloadRecord) =>
        send(async () => {
          try {
            await api.openFile(d.id, false);
          } catch (e) {
            if (!(e instanceof ApiError) || e.code !== 'CONFIRMATION_REQUIRED') throw e;
            const ok = await confirmAction({
              title: 'Open a program file?',
              description: `“${displayName(d)}” can run code on your computer. Only open it if you trust where it came from.`,
              confirmLabel: 'Open anyway',
              destructive: true,
            });
            if (ok) await api.openFile(d.id, true);
          }
        }, OPEN_FAILED),
      openFolder: (id: string) => send(() => api.openFolder(id), OPEN_FAILED),
      setSpeedLimit: (id: string, kib: number) => send(() => api.setSpeedLimit(id, kib)),
      setConnections: (id: string, connections: number) =>
        send(async () => {
          const applied = await api.setConnections(id, connections);
          toast.success(applied === connections ? `Connections set to ${String(applied)}` : `Asked for ${String(connections)} connections; ${String(applied)} apply`);
        }),
      setCategory: (id: string, categoryId: string | null) => send(() => api.setCategory(id, categoryId)),
      // A torrent's (#63).
      reannounce: (id: string) => send(async () => { await api.reannounce(id); toast.success('Asked the trackers for peers'); }),
      setUploadLimit: (id: string, kib: number) => send(() => api.setUploadLimit(id, kib)),
      /** Changes a torrent's files; whether it was done. */
      async changeFiles(id: string, files: number[], deleteDropped: boolean) {
        let ok = false;
        await send(async () => {
          reportKept(await api.chooseTorrentFiles(id, files, deleteDropped));
          toast.success('File choice changed');
          ok = true;
        });
        return ok;
      },
      copyLink: (d: DownloadRecord) => copy(d.sourceUrl, 'Link'),
      copyName: (d: DownloadRecord) => copy(displayName(d), 'Name'),
      pauseAll: () => send(async () => { await api.pauseAll(); toast.success('Paused all downloads'); }),
      resumeAll: () => send(async () => { await api.resumeAll(); toast.success('Resumed all downloads'); }),
      retryFailed: () => send(async () => { await api.retryFailed(); toast.success('Retrying the failed downloads'); }),
      clearCompleted: () =>
        send(async () => {
          const n = await api.clearCompleted();
          toast.success(n > 0 ? `Cleared ${plural(n, 'completed download')} from the list. Their files are kept.` : 'No completed downloads to clear');
        }),
    };
  }, [askLogin]);
  return { actions, busy: pending > 0 };
}

export type DownloadActions = ReturnType<typeof useDownloadActions>['actions'];
