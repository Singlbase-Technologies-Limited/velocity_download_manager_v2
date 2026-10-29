/**
 * Optional content script, registered only while "Send magnet links and .torrent
 * downloads to Velocity" is on and the user granted access to pages. It catches plain
 * clicks on magnet links and hands the link to the background. It reads nothing else
 * from the page, and Velocity always asks before adding the torrent.
 */
import { ext } from './shared/ext';

document.addEventListener(
  'click',
  (e) => {
    if (e.defaultPrevented || e.button !== 0 || e.ctrlKey || e.metaKey || e.shiftKey || e.altKey) return;
    const a = (e.target as Element | null)?.closest?.('a[href]');
    const href = a?.getAttribute('href') ?? '';
    if (!/^magnet:\?/i.test(href) || href.length > 8192) return;
    e.preventDefault();
    void ext.runtime.sendMessage({ type: 'magnet', url: href }).catch(() => undefined);
  },
  true,
);
