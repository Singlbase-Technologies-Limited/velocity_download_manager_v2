import { QueryClient, useQuery } from '@tanstack/react-query';
import { api } from './api';

/** The query keys for what the native side owns. */
export const keys = {
  appInfo: ['app-info'] as const,
  shellSettings: ['shell-settings'] as const,
  categories: ['categories'] as const,
};

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
