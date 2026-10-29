import { Layers } from 'lucide-react';
import { EmptyState } from '../../components/EmptyState';
import type { ListView } from '../../downloads/views';

const TITLES: Record<ListView, string> = {
  all: 'All Downloads',
  downloading: 'Downloading',
  queued: 'Queued',
  completed: 'Completed',
  seeding: 'Seeding',
  failed: 'Failed',
};

/** Where the download list goes. The list itself is #59's, so this says so instead of showing an empty list. */
export function DownloadsView({ view }: { view: ListView }) {
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex h-12 shrink-0 items-center border-b border-border bg-surface px-4">
        <h1 className="text-[13px] font-semibold">{TITLES[view]}</h1>
      </div>
      <EmptyState
        icon={Layers}
        title="Not available yet"
        description="The download list comes in a later update (#59). The counts beside each list are already real."
      />
    </div>
  );
}
