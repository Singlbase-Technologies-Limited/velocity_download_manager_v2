// Ported from V1 (src/components/ui/splitter.tsx) for #29.
import { useEffect, useRef, type KeyboardEvent, type PointerEvent } from 'react';
import { cn } from '../lib/cn';

interface Props {
  /** Accessible name, e.g. "Resize navigation". */
  label: string;
  /** id of the panel this separator resizes. */
  controls: string;
  /** Which side of the separator the panel is on: 'start' grows to the right, 'end' grows to the left. */
  side: 'start' | 'end';
  /** Current panel width in CSS px. */
  value: number;
  min: number;
  max: number;
  /** Called at most once per animation frame while dragging, with the clamped width. Write it to the DOM, not to React state. */
  onPreview?: (width: number) => void;
  /** Called once when a drag ends or a key is pressed: the clamped width and the unclamped one. */
  onCommit: (width: number, raw: number) => void;
  /** Enter collapses or expands the panel. */
  onToggle?: () => void;
  /** Double click restores the default width. */
  onReset?: () => void;
  className?: string;
}

const STEP = 16;
const BIG_STEP = 64;

interface Drag {
  pointerId: number;
  startX: number;
  startValue: number;
  raw: number;
  frame: number;
  restore: () => void;
}

/**
 * Vertical window splitter (WAI-ARIA "window splitter" pattern) shared by the navigation
 * sidebar and the details panel. Dragging uses pointer capture, so the drag keeps working
 * when the pointer leaves the handle or the window; widths are previewed once per frame
 * through `onPreview` and committed once on release, so a drag never re-renders the
 * download table on every pointer move or saves on every move.
 */
export function Splitter({ label, controls, side, value, min, max, onPreview, onCommit, onToggle, onReset, className }: Props) {
  const drag = useRef<Drag | null>(null);
  const ref = useRef<HTMLDivElement>(null);
  const clampW = (w: number) => Math.min(max, Math.max(min, w));

  // Tear a drag down if the component goes away mid-drag.
  useEffect(
    () => () => {
      const d = drag.current;
      if (d) {
        cancelAnimationFrame(d.frame);
        d.restore();
        drag.current = null;
      }
    },
    [],
  );

  const finish = (commit: boolean) => {
    const d = drag.current;
    if (!d) return;
    drag.current = null;
    cancelAnimationFrame(d.frame);
    d.restore();
    const el = ref.current;
    if (el) {
      delete el.dataset.dragging;
      try {
        if (el.hasPointerCapture(d.pointerId)) el.releasePointerCapture(d.pointerId);
      } catch {
        // The capture is already gone.
      }
    }
    if (commit) {
      onPreview?.(clampW(d.raw));
      onCommit(clampW(d.raw), d.raw);
    }
  };

  const onPointerDown = (e: PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0 || drag.current) return;
    // Stops text selection and focus changes under the pointer.
    e.preventDefault();
    const el = e.currentTarget;
    el.focus({ preventScroll: true });
    try {
      el.setPointerCapture(e.pointerId);
    } catch {
      // Synthetic pointers cannot be captured; the drag still works while over the handle.
    }
    const body = document.body.style;
    const prev = { cursor: body.cursor, userSelect: body.userSelect };
    body.cursor = 'col-resize';
    body.userSelect = 'none';
    const onBlur = () => { finish(true); };
    window.addEventListener('blur', onBlur);
    drag.current = {
      pointerId: e.pointerId,
      startX: e.clientX,
      startValue: value,
      raw: value,
      frame: 0,
      restore: () => {
        body.cursor = prev.cursor;
        body.userSelect = prev.userSelect;
        window.removeEventListener('blur', onBlur);
      },
    };
    el.dataset.dragging = 'true';
  };

  const onPointerMove = (e: PointerEvent<HTMLDivElement>) => {
    const d = drag.current;
    if (!d || e.pointerId !== d.pointerId) return;
    const dx = e.clientX - d.startX;
    d.raw = Math.round(d.startValue + (side === 'start' ? dx : -dx));
    if (!d.frame) {
      d.frame = requestAnimationFrame(() => {
        d.frame = 0;
        if (drag.current === d) onPreview?.(clampW(d.raw));
      });
    }
  };

  const onPointerUp = (e: PointerEvent<HTMLDivElement>) => {
    const d = drag.current;
    if (!d || e.pointerId !== d.pointerId) return;
    const dx = e.clientX - d.startX;
    d.raw = Math.round(d.startValue + (side === 'start' ? dx : -dx));
    finish(true);
  };

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    const step = e.shiftKey ? BIG_STEP : STEP;
    const grow = (dir: 1 | -1) => {
      const raw = value + dir * step;
      onCommit(clampW(raw), raw);
    };
    switch (e.key) {
      case 'ArrowRight':
        grow(side === 'start' ? 1 : -1);
        break;
      case 'ArrowLeft':
        grow(side === 'start' ? -1 : 1);
        break;
      case 'Home':
        onCommit(min, min);
        break;
      case 'End':
        onCommit(max, max);
        break;
      case 'Enter':
        if (!onToggle) return;
        onToggle();
        break;
      default:
        return;
    }
    e.preventDefault();
  };

  return (
    <div
      ref={ref}
      role="separator"
      aria-orientation="vertical"
      aria-label={label}
      aria-controls={controls}
      aria-valuenow={Math.round(value)}
      aria-valuemin={min}
      aria-valuemax={max}
      tabIndex={0}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerCancel={() => { finish(true); }}
      onLostPointerCapture={() => { finish(true); }}
      onKeyDown={onKeyDown}
      onClick={(e) => { e.stopPropagation(); }}
      onDoubleClick={(e) => {
        e.stopPropagation();
        onReset?.();
      }}
      className={cn(
        // A 1px line with a 9px hit area; hover, focus and drag widen it and show a grip, so feedback is not color only.
        'group/split relative z-20 w-px shrink-0 cursor-col-resize touch-none select-none bg-border outline-none',
        'before:absolute before:inset-y-0 before:-left-1 before:-right-1 before:content-[""]',
        'hover:bg-accent/60 focus-visible:bg-accent data-[dragging]:bg-accent',
        className,
      )}
    >
      <span
        aria-hidden
        className={cn(
          'pointer-events-none absolute left-1/2 top-1/2 h-8 w-1.5 -translate-x-1/2 -translate-y-1/2 rounded-full border border-border-strong bg-surface opacity-0 shadow-xs transition-opacity',
          'group-hover/split:opacity-100 group-focus-visible/split:border-accent group-focus-visible/split:opacity-100 group-focus-visible/split:ring-2 group-focus-visible/split:ring-ring/40 group-data-[dragging]/split:border-accent group-data-[dragging]/split:opacity-100',
        )}
      />
    </div>
  );
}
