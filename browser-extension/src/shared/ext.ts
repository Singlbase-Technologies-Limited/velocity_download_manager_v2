/** The WebExtension namespace: `browser` in Firefox (promise-based), `chrome` in Chromium (MV3 promises). */
export const ext: typeof chrome = (globalThis as { browser?: typeof chrome }).browser ?? chrome;

export async function send<T>(message: unknown): Promise<T> {
  const reply: unknown = await ext.runtime.sendMessage(message);
  return reply as T;
}
