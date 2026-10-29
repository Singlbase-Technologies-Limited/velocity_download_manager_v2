import { act, fireEvent, render, screen } from '@testing-library/react';
import type { ComponentProps } from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { Splitter } from './Splitter';

const nextFrame = () => act(() => new Promise<void>((r) => requestAnimationFrame(() => { r(); })));

function setup(props: Partial<ComponentProps<typeof Splitter>> = {}) {
  const onPreview = vi.fn();
  const onCommit = vi.fn();
  const onToggle = vi.fn();
  const onReset = vi.fn();
  const parentClick = vi.fn();
  const utils = render(
    <div onClick={parentClick}>
      <Splitter label="Resize panel" controls="panel" side="start" value={240} min={180} max={360} onPreview={onPreview} onCommit={onCommit} onToggle={onToggle} onReset={onReset} {...props} />
    </div>,
  );
  const handle = screen.getByRole('separator', { name: 'Resize panel' });
  const capture = vi.fn();
  const release = vi.fn();
  const own = { setPointerCapture: capture, releasePointerCapture: release, hasPointerCapture: () => capture.mock.calls.length > release.mock.calls.length };
  for (const [k, v] of Object.entries(own)) Object.defineProperty(handle, k, { configurable: true, value: v });
  return { ...utils, handle, onPreview, onCommit, onToggle, onReset, parentClick, capture, release };
}

beforeEach(() => {
  document.body.style.cursor = '';
  document.body.style.userSelect = '';
});

afterEach(() => vi.restoreAllMocks());

describe('Splitter accessibility', () => {
  it('is a focusable vertical separator with its value and bounds', () => {
    const { handle } = setup();
    expect(handle.getAttribute('aria-orientation')).toBe('vertical');
    expect(handle.getAttribute('aria-valuenow')).toBe('240');
    expect(handle.getAttribute('aria-valuemin')).toBe('180');
    expect(handle.getAttribute('aria-valuemax')).toBe('360');
    expect(handle.getAttribute('aria-controls')).toBe('panel');
    expect(handle.getAttribute('tabindex')).toBe('0');
  });
});

describe('Splitter keyboard', () => {
  it('moves with the arrow keys, faster with Shift, for a panel on the left', () => {
    const { handle, onCommit } = setup();
    fireEvent.keyDown(handle, { key: 'ArrowRight' });
    expect(onCommit).toHaveBeenLastCalledWith(256, 256);
    fireEvent.keyDown(handle, { key: 'ArrowLeft' });
    expect(onCommit).toHaveBeenLastCalledWith(224, 224);
    fireEvent.keyDown(handle, { key: 'ArrowRight', shiftKey: true });
    expect(onCommit).toHaveBeenLastCalledWith(304, 304);
  });

  it('grows a panel on the right with ArrowLeft', () => {
    const { handle, onCommit } = setup({ side: 'end' });
    fireEvent.keyDown(handle, { key: 'ArrowLeft' });
    expect(onCommit).toHaveBeenLastCalledWith(256, 256);
    fireEvent.keyDown(handle, { key: 'ArrowRight' });
    expect(onCommit).toHaveBeenLastCalledWith(224, 224);
  });

  it('jumps to the bounds with Home and End and clamps at the edges', () => {
    const { handle, onCommit } = setup({ value: 352 });
    fireEvent.keyDown(handle, { key: 'Home' });
    expect(onCommit).toHaveBeenLastCalledWith(180, 180);
    fireEvent.keyDown(handle, { key: 'End' });
    expect(onCommit).toHaveBeenLastCalledWith(360, 360);
    fireEvent.keyDown(handle, { key: 'ArrowRight' });
    expect(onCommit).toHaveBeenLastCalledWith(360, 368);
  });

  it('toggles the panel with Enter and ignores other keys', () => {
    const { handle, onToggle, onCommit } = setup();
    fireEvent.keyDown(handle, { key: 'Enter' });
    expect(onToggle).toHaveBeenCalledTimes(1);
    fireEvent.keyDown(handle, { key: 'a' });
    expect(onCommit).not.toHaveBeenCalled();
  });
});

describe('Splitter dragging', () => {
  it('previews on animation frames, captures the pointer and commits once on release', async () => {
    const { handle, onPreview, onCommit, capture, release } = setup();
    fireEvent.pointerDown(handle, { pointerId: 7, button: 0, clientX: 100 });
    expect(capture).toHaveBeenCalledWith(7);
    expect(handle.getAttribute('data-dragging')).toBe('true');
    expect(document.body.style.cursor).toBe('col-resize');
    expect(document.body.style.userSelect).toBe('none');

    fireEvent.pointerMove(handle, { pointerId: 7, clientX: 120 });
    fireEvent.pointerMove(handle, { pointerId: 7, clientX: 140 });
    await nextFrame();
    // Several moves in one frame give one preview with the latest width.
    expect(onPreview).toHaveBeenCalledTimes(1);
    expect(onPreview).toHaveBeenLastCalledWith(280);
    expect(onCommit).not.toHaveBeenCalled();

    fireEvent.pointerUp(handle, { pointerId: 7, clientX: 150 });
    expect(onCommit).toHaveBeenCalledTimes(1);
    expect(onCommit).toHaveBeenLastCalledWith(290, 290);
    expect(release).toHaveBeenCalledWith(7);
    expect(handle.hasAttribute('data-dragging')).toBe(false);
    expect(document.body.style.cursor).toBe('');
    expect(document.body.style.userSelect).toBe('');
  });

  it('clamps the preview but reports the raw width on release', async () => {
    const { handle, onPreview, onCommit } = setup({ side: 'end' });
    fireEvent.pointerDown(handle, { pointerId: 1, button: 0, clientX: 500 });
    fireEvent.pointerMove(handle, { pointerId: 1, clientX: 100 });
    await nextFrame();
    expect(onPreview).toHaveBeenLastCalledWith(360);
    fireEvent.pointerUp(handle, { pointerId: 1, clientX: 100 });
    expect(onCommit).toHaveBeenLastCalledWith(360, 640);
  });

  it('finishes the drag when the pointer capture is lost', () => {
    const { handle, onCommit } = setup();
    fireEvent.pointerDown(handle, { pointerId: 2, button: 0, clientX: 100 });
    fireEvent.pointerMove(handle, { pointerId: 2, clientX: 90 });
    fireEvent(handle, new Event('lostpointercapture', { bubbles: true }));
    expect(onCommit).toHaveBeenCalledWith(230, 230);
    expect(document.body.style.cursor).toBe('');
    fireEvent.pointerMove(handle, { pointerId: 2, clientX: 10 });
    fireEvent.pointerUp(handle, { pointerId: 2, clientX: 10 });
    expect(onCommit).toHaveBeenCalledTimes(1);
  });

  it('ignores secondary buttons and does not click through', () => {
    const { handle, onCommit, capture, parentClick } = setup();
    fireEvent.pointerDown(handle, { pointerId: 3, button: 2, clientX: 100 });
    expect(capture).not.toHaveBeenCalled();
    fireEvent.pointerUp(handle, { pointerId: 3, clientX: 200 });
    expect(onCommit).not.toHaveBeenCalled();
    fireEvent.click(handle);
    expect(parentClick).not.toHaveBeenCalled();
  });

  it('cleans up when unmounted mid-drag and leaves no window listeners behind', async () => {
    const add = vi.spyOn(window, 'addEventListener');
    const remove = vi.spyOn(window, 'removeEventListener');
    const { handle, onPreview, unmount } = setup();
    for (let i = 0; i < 3; i += 1) {
      fireEvent.pointerDown(handle, { pointerId: 4, button: 0, clientX: 100 });
      fireEvent.pointerUp(handle, { pointerId: 4, clientX: 110 });
    }
    fireEvent.pointerDown(handle, { pointerId: 4, button: 0, clientX: 100 });
    fireEvent.pointerMove(handle, { pointerId: 4, clientX: 160 });
    // Each finished drag previews its final width once; the unfinished one must not.
    expect(onPreview).toHaveBeenCalledTimes(3);
    unmount();
    await nextFrame();
    expect(onPreview).toHaveBeenCalledTimes(3);
    expect(document.body.style.cursor).toBe('');
    expect(document.body.style.userSelect).toBe('');
    const count = (spy: typeof add, type: string) => spy.mock.calls.filter((c) => c[0] === type).length;
    expect(count(add, 'blur')).toBe(count(remove, 'blur'));
  });

  it('resets on double click', () => {
    const { handle, onReset } = setup();
    fireEvent.doubleClick(handle);
    expect(onReset).toHaveBeenCalledTimes(1);
  });
});
