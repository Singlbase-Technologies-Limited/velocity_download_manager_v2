import { QueryClientProvider, useQuery, type QueryClient } from '@tanstack/react-query';
import { MonitorX } from 'lucide-react';
import type { ReactNode } from 'react';
import { Toaster } from 'sonner';
import { ConfirmHost } from '../components/ConfirmHost';
import { Spinner } from '../components/Spinner';
import { Onboarding } from '../features/onboarding/Onboarding';
import { api, isDesktop } from '../lib/api';
import { toApiError } from '../lib/errors';
import { createQueryClient, keys, useShellSettings } from '../lib/queries';
import { FatalError } from './FatalError';
import { Shell } from './Shell';
import { useSnapshotFeed } from './useSnapshotFeed';
import { useTheme } from './useTheme';

export function App({ client = createQueryClient() }: { client?: QueryClient }) {
  return (
    <QueryClientProvider client={client}>
      <Root />
      <ConfirmHost />
    </QueryClientProvider>
  );
}

function Centered({ children }: { children: ReactNode }) {
  return <div className="flex h-full items-center justify-center p-8">{children}</div>;
}

function Root() {
  const desktop = isDesktop();
  const info = useQuery({ queryKey: keys.appInfo, queryFn: api.appInfo, retry: false, enabled: desktop });
  const settings = useShellSettings(desktop && info.isSuccess);
  useSnapshotFeed(desktop && info.isSuccess);
  const theme = settings.data?.appearance.theme ?? 'system';
  useTheme(theme);

  let content: ReactNode;
  if (!desktop) {
    content = (
      <Centered>
        <div className="flex max-w-md flex-col items-center gap-3 text-center">
          <MonitorX className="size-10 text-fg-subtle" aria-hidden />
          <h1 className="text-base font-semibold">Desktop application required</h1>
          <p className="text-[13px] text-fg-muted">
            This window controls the downloads on this computer, so it runs only inside the Velocity desktop app. Start it with <code className="font-mono">pnpm dev</code>.
          </p>
        </div>
      </Centered>
    );
  } else if (info.isError) {
    content = <FatalError title="Velocity couldn't start" error={toApiError(info.error)} />;
  } else if (settings.isError) {
    content = <FatalError title="Velocity couldn't read its settings" error={toApiError(settings.error)} onRetry={() => void settings.refetch()} />;
  } else if (!info.data || !settings.data) {
    content = (
      <Centered>
        <div className="flex items-center gap-3 text-fg-muted" role="status">
          <Spinner />
          <span>Starting Velocity…</span>
        </div>
      </Centered>
    );
  } else if (!settings.data.onboardingCompleted) {
    content = <Onboarding app={info.data} defaultDirectory={settings.data.defaultDirectory ?? ''} />;
  } else {
    content = <Shell app={info.data} appearance={settings.data.appearance} />;
  }
  return (
    <>
      {content}
      <Toaster position="bottom-right" theme={theme} richColors closeButton duration={3500} />
    </>
  );
}
