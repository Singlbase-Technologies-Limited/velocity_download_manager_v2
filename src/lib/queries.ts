import { QueryClient, useQuery } from '@tanstack/react-query';
import { api } from './api';

/** The query keys for what the native side owns. */
export const keys = {
  appInfo: ['app-info'] as const,
  shellSettings: ['shell-settings'] as const,
  categories: ['categories'] as const,
  capabilities: ['capabilities'] as const,
  settings: ['settings'] as const,
  torrent: (id: string) => ['torrent', id] as const,
  trackers: (id: string) => ['torrent', id, 'trackers'] as const,
  peers: (id: string) => ['torrent', id, 'peers'] as const,
};

/** How often a shown torrent's swarm is asked for (PO decision on #63): only while it is shown. */
export const SWARM_EVERY_MS = 1000;

/** V1's query defaults: no refetching on focus, one retry for queries, none for commands. */
export function createQueryClient(): QueryClient {
  return new QueryClient({
    defaultOptions: { queries: { refetchOnWindowFocus: false, retry: 1 }, mutations: { retry: false } },
  });
}

export function useShellSettings(enabled = true) {
  return useQuery({ queryKey: keys.shellSettings, queryFn: api.shellSettings, retry: false, enabled });
}

export function useCategories() {
  return useQuery({ queryKey: keys.categories, queryFn: api.categories });
}

/** What the transfer engine can do (#57): the Add dialog offers only that. */
export function useCapabilities() {
  return useQuery({ queryKey: keys.capabilities, queryFn: api.capabilities });
}

/** A torrent's seeding, upload limit and files (#63); fetched for the torrent shown, and again after a change. */
export function useTorrentDetails(id: string, enabled = true) {
  return useQuery({ queryKey: keys.torrent(id), queryFn: () => api.torrentDetails(id), retry: false, enabled });
}

/** A shown torrent's trackers and peers, asked for every second while `live` (PO decision on #63). */
export function useSwarm(id: string, live: boolean) {
  const options = { retry: false, enabled: live, refetchInterval: live ? SWARM_EVERY_MS : false } as const;
  const trackers = useQuery({ queryKey: keys.trackers(id), queryFn: () => api.trackers(id), ...options });
  const peers = useQuery({ queryKey: keys.peers(id), queryFn: () => api.peers(id), ...options });
  return { trackers, peers };
}

/** Every setting the Settings screen edits (#30). */
export function useSettings() {
  return useQuery({ queryKey: keys.settings, queryFn: api.settings, retry: false });
}
