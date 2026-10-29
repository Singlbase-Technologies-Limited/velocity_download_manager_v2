import { describe, expect, it } from 'vitest';
import type { TrackerStatus } from '../../contract';
import { buildFileTree, fileChoice, folderState, portMapText, sameSelection, seedingSummary, selectedSize, toggleFolder, torrentLinksInPaste, trackerSummary } from './torrentLogic';

// V1's tests/unit/torrentLogic.test.ts, ported (#63); port mapping's text came with the BitTorrent settings (#30).

function must<T>(v: T | undefined): T {
  if (v === undefined) throw new Error('missing');
  return v;
}

const files = [
  { index: 1, path: 'album/a.bin', length: 100 },
  { index: 2, path: 'album/sub/b.bin', length: 20 },
  { index: 3, path: 'album/sub/deep/c.bin', length: 5 },
  { index: 4, path: 'album/z.txt', length: 1 },
];

describe('torrent file tree', () => {
  it('nests files in folders with folder sizes, folders first', () => {
    const root = buildFileTree(files);
    expect(root.children.map((c) => c.name)).toEqual(['album']);
    const album = must(root.children[0]);
    expect(album.size).toBe(126);
    expect(album.children.map((c) => c.name)).toEqual(['sub', 'a.bin', 'z.txt']);
    const sub = must(album.children[0]);
    expect(sub.size).toBe(25);
    expect(sub.fileIndexes).toEqual([2, 3]);
  });

  it('selects and clears whole folders and reports partial state', () => {
    const album = must(buildFileTree(files).children[0]);
    const sub = must(album.children[0]);
    let sel = new Set<number>([1, 2, 3, 4]);
    sel = toggleFolder(sel, sub);
    expect([...sel].sort()).toEqual([1, 4]);
    expect(folderState(sub, sel)).toBe('none');
    expect(folderState(album, sel)).toBe('some');
    sel = toggleFolder(sel, album);
    expect(folderState(album, sel)).toBe('all');
    sel = new Set([2]);
    // A partly selected folder becomes fully selected first.
    expect([...toggleFolder(sel, sub)].sort()).toEqual([2, 3]);
  });

  it('counts only the selected files', () => {
    expect(selectedSize(files, new Set([2, 3]))).toBe(25);
    expect(selectedSize(files, new Set())).toBe(0);
  });

  it('sends no choice when every file is chosen and refuses an empty one', () => {
    expect(fileChoice(files, new Set([1, 2, 3, 4]))).toEqual({ files: null, error: null });
    expect(fileChoice(files, new Set([3, 1]))).toEqual({ files: [1, 3], error: null });
    expect(fileChoice(files, new Set()).error).toBe('Choose at least one file.');
  });

  it('handles single-file torrents and odd paths', () => {
    const root = buildFileTree([{ index: 1, path: 'movie.mkv', length: 9 }]);
    expect(root.children).toHaveLength(1);
    expect(root.children[0]).toMatchObject({ name: 'movie.mkv', isFile: true, fileIndexes: [1] });
  });
});

describe('torrent sources', () => {
  it('picks up pasted magnet and .torrent links only', () => {
    expect(torrentLinksInPaste('magnet:?xt=urn:btih:abc\nhttps://x.test/a.torrent')).toEqual(['magnet:?xt=urn:btih:abc', 'https://x.test/a.torrent']);
    expect(torrentLinksInPaste('magnet:?xt=urn:btih:abc https://x.test/file.zip')).toEqual([]);
    expect(torrentLinksInPaste('   ')).toEqual([]);
    expect(torrentLinksInPaste('hello world')).toEqual([]);
  });

  it('describes seeding policies in plain words', () => {
    expect(seedingSummary({ mode: 'never' })).toBe('Stops when the download finishes');
    expect(seedingSummary({ mode: 'time', minutes: 90 })).toBe('Seeds for 1 h 30 min');
    expect(seedingSummary({ mode: 'ratio', ratio: 1.5 })).toBe('Seeds until the share ratio reaches 1.5');
    expect(seedingSummary({ mode: 'timeOrRatio', minutes: 30, ratio: 2 })).toBe('Seeds for 30 min or until the share ratio reaches 2');
    expect(seedingSummary({ mode: 'unlimited' })).toBe('Seeds until you stop it');
  });
});

describe('tracker text', () => {
  const tr: TrackerStatus = { url: 'udp://t.test:1', tier: 0, state: 'error', message: 'unregistered torrent', fails: 3, nextAnnounceSecs: 45, seeds: null, peers: null };
  it('summarises a tracker without inventing counts', () => {
    expect(trackerSummary(tr)).toBe('unregistered torrent · failed 3 times · next announce in 45 s');
    expect(trackerSummary({ ...tr, state: 'working', message: null, fails: 0, nextAnnounceSecs: 1800, seeds: 2, peers: 7 })).toBe('2 seeds, 7 peers · next announce in 30 min');
    // A tracker not contacted yet has nothing to report.
    expect(trackerSummary({ ...tr, state: 'notContacted', message: null, fails: 0, nextAnnounceSecs: null })).toBe('');
  });

  it('compares file selections as sets', () => {
    expect(sameSelection(new Set([1, 2]), new Set([2, 1]))).toBe(true);
    expect(sameSelection(new Set([1]), new Set([1, 2]))).toBe(false);
  });
});

describe('port mapping text (#30)', () => {
  const at = (portMapping: Parameters<typeof portMapText>[0]['portMapping']) => portMapText({ listenPort: 6882, listenError: null, portMapping });
  it('describes every port mapping state', () => {
    expect(at({ state: 'mapped', externalPort: 40000, via: 'natpmp' })).toBe('Port 6882 forwarded by the router as 40000 (NAT-PMP).');
    expect(at({ state: 'off' })).toMatch(/^Off/);
    expect(at({ state: 'failed', message: 'conflict' })).toBe('The router refused: conflict');
    expect(at({ state: 'noRouter' })).toMatch(/No router answered/);
    expect(at({ state: 'searching' })).toMatch(/Looking for a router/);
  });
});
