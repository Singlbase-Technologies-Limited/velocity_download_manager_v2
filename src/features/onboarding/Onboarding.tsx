import { useQueryClient } from '@tanstack/react-query';
import { CheckCircle2, FolderOpen, ShieldCheck, XCircle } from 'lucide-react';
import { useState } from 'react';
import { BrandIcon } from '../../components/BrandIcon';
import { Button } from '../../components/Button';
import { Input } from '../../components/Input';
import { Spinner } from '../../components/Spinner';
import type { AppInfo, EngineHealth } from '../../contract';
import { api } from '../../lib/api';
import { ERROR_MESSAGES, toApiError } from '../../lib/errors';
import { keys } from '../../lib/queries';
import { useDownloadsStore } from '../../stores/downloads';

function EngineStatus({ health }: { health: EngineHealth | undefined }) {
  const [icon, title, detail] =
    health?.state === 'running'
      ? [<CheckCircle2 key="i" className="mt-0.5 size-4 shrink-0 text-success" aria-hidden />, 'The transfer engine is running', 'It runs on this computer, as its own process.']
      : health?.state === 'unavailable'
        ? [<XCircle key="i" className="mt-0.5 size-4 shrink-0 text-danger" aria-hidden />, 'The transfer engine isn’t available', ERROR_MESSAGES[health.code]]
        : [<Spinner key="i" className="mt-0.5" />, 'Starting the transfer engine…', 'This takes a moment.'];
  return (
    <div className="mt-2 flex items-start gap-2 rounded-lg bg-surface-2 px-3 py-2.5 text-xs" role="status" aria-label="Transfer engine">
      {icon}
      <div className="min-w-0">
        <p className="font-medium">{title}</p>
        <p className="mt-0.5 text-fg-muted">{detail}</p>
      </div>
    </div>
  );
}

/**
 * The first-run setup (V1's Onboarding): the engine's state and the folder for downloads. Unlike V1 it
 * doesn't wait for the engine: V2's window works without one and says why (#58). The browser extension
 * step comes with #64.
 */
export function Onboarding({ app, defaultDirectory }: { app: AppInfo; defaultDirectory: string }) {
  const qc = useQueryClient();
  const engine = useDownloadsStore((s) => s.snapshot?.engine);
  const [directory, setDirectory] = useState(defaultDirectory);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  const browse = async () => {
    setError(null);
    try {
      const chosen = await api.chooseDirectory(directory.trim() || null);
      if (chosen) setDirectory(chosen);
    } catch (e) {
      setError(toApiError(e).message);
    }
  };

  const finish = async () => {
    setError(null);
    setSaving(true);
    try {
      qc.setQueryData(keys.shellSettings, await api.completeOnboarding(directory.trim()));
    } catch (e) {
      setError(toApiError(e).message);
      setSaving(false);
    }
  };

  return (
    <div className="flex h-full items-center justify-center overflow-y-auto bg-bg p-8">
      <div className="w-full max-w-xl rounded-2xl border border-border bg-surface p-8 shadow-panel">
        <div className="flex items-center gap-3">
          <BrandIcon className="size-10 shrink-0 drop-shadow-sm" />
          <div>
            <h1 className="text-lg font-semibold">Welcome to {app.name}</h1>
            <p className="text-xs text-fg-muted">Version {app.version}</p>
          </div>
        </div>

        <ol className="mt-6 space-y-5">
          <li>
            <h2 className="text-[13px] font-semibold">1. Download engine</h2>
            <EngineStatus health={engine} />
          </li>
          <li>
            <h2 className="text-[13px] font-semibold">2. Where should downloads go?</h2>
            <div className="mt-2 flex gap-2">
              <Input aria-label="Download folder" value={directory} onChange={(e) => { setDirectory(e.target.value); }} placeholder="Choose a folder" />
              <Button variant="secondary" onClick={() => void browse()}>
                <FolderOpen aria-hidden /> Browse
              </Button>
            </div>
            <p className="mt-1.5 text-xs text-fg-subtle">Choose a folder that exists. You can change it later in Settings.</p>
          </li>
        </ol>

        <p className="mt-6 flex items-start gap-2 text-xs text-fg-muted">
          <ShieldCheck className="mt-0.5 size-4 shrink-0 text-success" aria-hidden />
          No analytics or telemetry. Your downloads, history and settings stay on this computer.
        </p>

        {error ? (
          <p role="alert" className="mt-4 text-xs text-danger">
            {error}
          </p>
        ) : null}

        <div className="mt-6 flex justify-end">
          <Button variant="primary" size="lg" busy={saving} disabled={!directory.trim()} onClick={() => void finish()}>
            Get started
          </Button>
        </div>
      </div>
    </div>
  );
}
