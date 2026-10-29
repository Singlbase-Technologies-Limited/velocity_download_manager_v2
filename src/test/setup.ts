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
const stub = (name: 'scrollIntoView' | 'hasPointerCapture' | 'releasePointerCapture', value: () => unknown) => {
  if (!(name in Element.prototype)) Object.defineProperty(Element.prototype, name, { configurable: true, value });
};
stub('scrollIntoView', () => undefined);
stub('hasPointerCapture', () => false);
stub('releasePointerCapture', () => undefined);

afterEach(() => {
  cleanup();
  clearMocks();
});
