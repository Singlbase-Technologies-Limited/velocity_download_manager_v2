import { useQueryClient } from '@tanstack/react-query';
import { useState } from 'react';
import { toast } from 'sonner';
import { Button } from '../../components/Button';
import { Dialog } from '../../components/Dialog';
import { Spinner } from '../../components/Spinner';
import { api } from '../../lib/api';
import { toApiError } from '../../lib/errors';
import { formatBytes } from '../../lib/format';
import { keys, useTorrentDetails } from '../../lib/queries';
import { useUiStore } from '../../stores/ui';
import { FileTree } from './FileTree';
import { fileChoice, selectedSize } from './torrentLogic';

/** Chooses the files of a torrent whose metadata just arrived, in Choose files (V1's ChooseFilesDialog, #63). */
export function ChooseFilesDialog() {
  const id = useUiStore((s) => s.chooseFilesId);
  const close = useUiStore((s) => s.closeChooseFiles);
  return id ? <ChooseFilesForm key={id} id={id} onClose={close} /> : null;
}

function ChooseFilesForm({ id, onClose }: { id: string; onClose: () => void }) {
  const qc = useQueryClient();
  const details = useTorrentDetails(id);
  const contents = details.data?.contents ?? null;
  const files = contents?.files ?? [];
  const [selected, setSelected] = useState<Set<number> | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Everything is chosen until the user changes it.
  const sel = selected ?? new Set(files.map((f) => f.index));
  const start = async () => {
    const choice = fileChoice(files, sel);
    if (choice.error !== null) {
      setError(choice.error);
      return;
    }
    setBusy(true);
    try {
      await api.chooseTorrentFiles(id, choice.files ?? files.map((f) => f.index), false);
      void qc.invalidateQueries({ queryKey: keys.torrent(id) });
      toast.success('Download started');
      onClose();
    } catch (e) {
      setError(toApiError(e).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog
      open
      onOpenChange={(o) => { if (!o) onClose(); }}
      title="Choose files to download"
      description={contents?.name}
      className="max-w-2xl"
      footer={
        <>
          <span className="mr-auto text-xs tabular-nums text-fg-muted">
            {sel.size} of {files.length} files · {formatBytes(selectedSize(files, sel))}
          </span>
          <Button variant="ghost" onClick={onClose}>
            Later
          </Button>
          <Button variant="primary" busy={busy} disabled={files.length === 0} onClick={() => void start()}>
            Start download
          </Button>
        </>
      }
    >
      {details.isPending ? (
        <div className="flex items-center gap-2 text-fg-muted">
          <Spinner /> Loading files…
        </div>
      ) : details.isError ? (
        <p role="alert" className="text-xs text-danger">
          {toApiError(details.error).message}
        </p>
      ) : contents === null ? (
        <p className="text-xs text-fg-muted">The torrent’s metadata hasn’t arrived yet, so there are no files to choose.</p>
      ) : (
        <div className="space-y-3">
          <div className="flex gap-2 text-xs">
            <Button size="sm" variant="ghost" onClick={() => { setSelected(new Set(files.map((f) => f.index))); }}>
              Select all
            </Button>
            <Button size="sm" variant="ghost" onClick={() => { setSelected(new Set()); }}>
              Select none
            </Button>
          </div>
          <FileTree files={files} selected={sel} onChange={setSelected} />
          <p className="text-xs text-fg-muted">You can change the files later, in the download’s details. Files you skip may still be partly written where pieces overlap.</p>
          {error ? (
            <p role="alert" className="text-xs text-danger">
              {error}
            </p>
          ) : null}
        </div>
      )}
    </Dialog>
  );
}
