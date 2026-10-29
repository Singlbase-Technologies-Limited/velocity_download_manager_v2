import { ShieldAlert, ShieldQuestion } from 'lucide-react';
import { useMemo, useState } from 'react';
import { toast } from 'sonner';
import { Button } from '../../components/Button';
import { Dialog } from '../../components/Dialog';
import { api } from '../../lib/api';
import { toApiError } from '../../lib/errors';
import { useDownloadsStore } from '../../stores/downloads';

/**
 * The SFTP host key question (PO decision on #60): an SFTP download is added, connects, and stops
 * before logging in when the server's key isn't trusted yet (#53). This asks once per download and
 * key: "Trust and continue" trusts the key and retries the download; "Cancel" leaves it failed. A
 * key that changed since it was trusted is always refused, so it is only explained.
 */
export function HostKeyPrompt() {
  const downloads = useDownloadsStore((s) => s.snapshot?.downloads);
  const [answered, setAnswered] = useState<ReadonlySet<string>>(new Set());
  const [busy, setBusy] = useState(false);
  const asking = useMemo(
    () =>
      downloads?.find((d) => {
        const key = d.error?.hostKey;
        return d.status === 'failed' && key && !answered.has(`${d.id} ${key.fingerprint}`);
      }),
    [downloads, answered],
  );
  const key = asking?.error?.hostKey;
  if (!asking || !key) return null;

  const done = () => { setAnswered((a) => new Set(a).add(`${asking.id} ${key.fingerprint}`)); };
  const trust = async () => {
    setBusy(true);
    try {
      await api.trustHostKey(key.host, key.port, key.fingerprint);
      await api.retryDownloads([asking.id]);
    } catch (e) {
      toast.error(toApiError(e).message);
    } finally {
      setBusy(false);
      done();
    }
  };
  const changed = key.pinned !== null;
  const server = (
    <dl className="mt-3 space-y-2 rounded-md bg-surface-2 px-3 py-2 text-xs">
      <div>
        <dt className="text-fg-muted">Server</dt>
        <dd className="font-mono">{`${key.host}:${String(key.port)}`}</dd>
      </div>
      <div>
        <dt className="text-fg-muted">{changed ? 'Key it shows now' : 'Its key’s fingerprint'}</dt>
        <dd className="break-all font-mono">{key.fingerprint}</dd>
      </div>
    </dl>
  );
  return (
    <Dialog
      open
      onOpenChange={(o) => { if (!o) done(); }}
      title={changed ? 'The server’s key has changed' : 'Trust this server?'}
      footer={
        changed ? (
          <Button variant="primary" onClick={done}>
            Done
          </Button>
        ) : (
          <>
            <Button variant="ghost" onClick={done}>
              Cancel
            </Button>
            <Button variant="primary" busy={busy} onClick={() => void trust()}>
              Trust and continue
            </Button>
          </>
        )
      }
    >
      <div className="flex gap-3 text-[13px]">
        {changed ? <ShieldAlert className="mt-0.5 size-5 shrink-0 text-danger" aria-hidden /> : <ShieldQuestion className="mt-0.5 size-5 shrink-0 text-accent" aria-hidden />}
        <div className="min-w-0">
          {changed ? (
            <p>
              This server shows a different key from the one you trusted. Someone may be pretending to be it, so Velocity won’t connect. If the server’s owner changed its key on purpose, remove the old one
              from Velocity’s known hosts before trying again.
            </p>
          ) : (
            <p>Velocity hasn’t connected to this server before. Check the fingerprint with the server’s owner, then trust it to continue the download.</p>
          )}
          {server}
        </div>
      </div>
    </Dialog>
  );
}
