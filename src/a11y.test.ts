import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';

// The accessibility audit's checks on the design tokens (#66, docs/ACCESSIBILITY.md): contrast in both themes,
// the focus ring, and reduced motion. Read from src/index.css itself, so a changed colour is checked as it ships.

const css = readFileSync(join(__dirname, 'index.css'), 'utf8');

function tokens(selector: string): Record<string, string> {
  const start = css.indexOf(`${selector} {`);
  const body = css.slice(start, css.indexOf('}', start));
  return Object.fromEntries([...body.matchAll(/--([\w-]+):\s*(#[0-9a-f]{6});/g)].map((m) => [m[1] ?? '', m[2] ?? '']));
}

/** WCAG 2.2's relative luminance and contrast ratio. */
function luminance(hex: string): number {
  const [r = 0, g = 0, b = 0] = [1, 3, 5].map((i) => {
    const c = parseInt(hex.slice(i, i + 2), 16) / 255;
    return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

export function contrast(a: string, b: string): number {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x) as [number, number];
  return (hi + 0.05) / (lo + 0.05);
}

/** Every background text is shown on, and the text colours shown on them. */
const BACKGROUNDS = ['bg', 'surface', 'surface-2', 'surface-3', 'sidebar', 'row-hover', 'row-selected'];
const TEXT = ['fg', 'fg-muted', 'fg-subtle', 'accent', 'success', 'warning', 'danger'];
/** Text on its own tinted background: badges, notices, the selected navigation item. */
const TINTED: [string, string][] = [
  ['accent', 'accent-soft'],
  ['success', 'success-soft'],
  ['warning', 'warning-soft'],
  ['danger', 'danger-soft'],
  ['accent-fg', 'accent'],
  ['accent-fg', 'accent-strong'],
];

describe.each([
  ['light', ':root'],
  ['dark', '.dark'],
])('the %s theme', (_name, selector) => {
  const t = tokens(selector);

  it('has every token the checks read', () => {
    for (const name of [...BACKGROUNDS, ...TEXT, ...TINTED.flat(), 'ring']) expect(t[name], name).toMatch(/^#[0-9a-f]{6}$/);
  });

  it('gives all text at least 4.5:1 against every background it is shown on (WCAG AA)', () => {
    const low = TEXT.flatMap((fg) => BACKGROUNDS.map((bg) => [fg, bg, contrast(t[fg] ?? '', t[bg] ?? '')] as const)).filter(([, , r]) => r < 4.5);
    expect(low.map(([fg, bg, r]) => `${fg} on ${bg}: ${r.toFixed(2)}`)).toEqual([]);
  });

  it('gives text on its tinted background at least 4.5:1', () => {
    const low = TINTED.map(([fg, bg]) => [fg, bg, contrast(t[fg] ?? '', t[bg] ?? '')] as const).filter(([, , r]) => r < 4.5);
    expect(low.map(([fg, bg, r]) => `${fg} on ${bg}: ${r.toFixed(2)}`)).toEqual([]);
  });

  it('draws the focus ring at least 3:1 against the surfaces (WCAG non-text contrast)', () => {
    for (const bg of BACKGROUNDS) expect(contrast(t.ring ?? '', t[bg] ?? ''), bg).toBeGreaterThanOrEqual(3);
  });
});

describe('the contrast formula', () => {
  it('matches WCAG’s known values', () => {
    expect(contrast('#000000', '#ffffff')).toBeCloseTo(21, 5);
    expect(contrast('#777777', '#ffffff')).toBeCloseTo(4.48, 2);
    expect(contrast('#ffffff', '#ffffff')).toBe(1);
  });
});

describe('motion and focus', () => {
  it('stops animations and transitions when the system asks for reduced motion', () => {
    const block = css.slice(css.indexOf('@media (prefers-reduced-motion: reduce)'));
    expect(block).toMatch(/animation-duration:\s*0\.01ms !important/);
    expect(block).toMatch(/transition-duration:\s*0\.01ms !important/);
  });

  it('outlines whatever has keyboard focus with the ring colour', () => {
    expect(css).toMatch(/:focus-visible\s*\{\s*outline:\s*2px solid var\(--ring\)/);
  });
});
