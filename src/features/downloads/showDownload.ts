import { useDownloadsStore } from '../../stores/downloads';
import { useUiStore } from '../../stores/ui';

/** Shows a download in the list: the whole list, without a search, with it selected and scrolled to. */
export function showDownload(id: string) {
  useUiStore.getState().setView('all');
  const list = useDownloadsStore.getState();
  list.setSearch('');
  list.select(id, 'single');
}
