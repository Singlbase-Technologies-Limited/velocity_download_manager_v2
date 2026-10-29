import { beforeEach, describe, expect, it } from 'vitest';
import { emptySnapshot } from '../test/fakeBackend';
import { makeRecord } from '../test/records';
import { INITIAL_LIST, useDownloadsStore } from './downloads';

const ids = ['a', 'b', 'c', 'd'];
const selected = () => [...useDownloadsStore.getState().selection].sort();

beforeEach(() => {
  useDownloadsStore.setState({ snapshot: null, error: null, ...INITIAL_LIST });
});

describe('selection (V1’s click rules)', () => {
  it('a click selects one, Ctrl toggles, Shift selects the range from the last click', () => {
    const { select } = useDownloadsStore.getState();
    select('b', 'single', ids);
    expect(selected()).toEqual(['b']);
    select('d', 'toggle', ids);
    expect(selected()).toEqual(['b', 'd']);
    select('d', 'toggle', ids);
    expect(selected()).toEqual(['b']);
    select('a', 'single', ids);
    select('c', 'range', ids);
    expect(selected()).toEqual(['a', 'b', 'c']);
    expect(useDownloadsStore.getState().focus).toBe('c');
  });

  it('select-all takes the ids shown, and clearing empties it', () => {
    const { selectAll, clearSelection } = useDownloadsStore.getState();
    selectAll(['a', 'c']);
    expect(selected()).toEqual(['a', 'c']);
    clearSelection();
    expect(selected()).toEqual([]);
    expect(useDownloadsStore.getState().focus).toBeNull();
  });

  it('a download that leaves the snapshot leaves the selection, so no action targets it', () => {
    const { selectAll, setSnapshot } = useDownloadsStore.getState();
    selectAll(['a', 'b']);
    setSnapshot({ ...emptySnapshot(), downloads: [makeRecord({ id: 'b' })] });
    expect(selected()).toEqual(['b']);
  });
});

describe('sorting', () => {
  it('starts newest first; a new column sorts by its natural direction, the same one flips', () => {
    const { toggleSort } = useDownloadsStore.getState();
    expect(useDownloadsStore.getState().sort).toEqual({ key: 'added', dir: 'desc' });
    toggleSort('name');
    expect(useDownloadsStore.getState().sort).toEqual({ key: 'name', dir: 'asc' });
    toggleSort('name');
    expect(useDownloadsStore.getState().sort).toEqual({ key: 'name', dir: 'desc' });
    toggleSort('size');
    expect(useDownloadsStore.getState().sort).toEqual({ key: 'size', dir: 'desc' });
  });
});
