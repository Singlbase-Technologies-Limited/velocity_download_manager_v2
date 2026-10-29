// Every UI test runs in jsdom (vite.config.ts), with Tauri's IPC mocked per test (#29).
import { clearMocks } from '@tauri-apps/api/mocks';
import { cleanup } from '@testing-library/react';
import { afterEach } from 'vitest';

// jsdom lacks the browser APIs Radix and the splitter use.
class ResizeObserverStub {
  observe() {}
  unobserve() {}
  disconnect() {}
}
if (!('ResizeObserver' in globalThis)) globalThis.ResizeObserver = ResizeObserverStub;
const stub = (name: 'scrollIntoView' | 'hasPointerCapture' | 'setPointerCapture' | 'releasePointerCapture', value: () => unknown) => {
  if (!(name in Element.prototype)) Object.defineProperty(Element.prototype, name, { configurable: true, value });
};
stub('scrollIntoView', () => undefined);
stub('hasPointerCapture', () => false);
stub('setPointerCapture', () => undefined);
stub('releasePointerCapture', () => undefined);

afterEach(() => {
  cleanup();
  clearMocks();
});

// jsdom does no layout; elements get a plausible size so the virtualised download list draws its rows (V1's setup).
for (const [name, px] of [['offsetHeight', 800], ['offsetWidth', 1200], ['clientHeight', 800], ['clientWidth', 1200]] as const) {
  Object.defineProperty(HTMLElement.prototype, name, { configurable: true, get: () => px });
}
