/** Native messaging transport with a timeout and error normalization. */
import { NATIVE_HOST_NAME, SHORT_NAME } from './branding';
import { parseResponse, PROTOCOL_VERSION, type Request, type Response } from './protocol';

export const NATIVE_TIMEOUT_MS = 10_000;

export interface NativeApi {
  sendNativeMessage(application: string, message: object): Promise<unknown>;
}

export async function sendNative(api: NativeApi, req: Request, timeoutMs = NATIVE_TIMEOUT_MS): Promise<Response> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const timeout = new Promise<Response>((resolve) => {
    timer = setTimeout(() => {
      resolve({ v: PROTOCOL_VERSION, id: req.id, ok: false, error: { code: 'TIMEOUT', message: `${SHORT_NAME} did not answer in time.` } });
    }, timeoutMs);
  });
  try {
    const call = api.sendNativeMessage(NATIVE_HOST_NAME, req).then(
      (raw) => parseResponse(raw, req.id),
      (e: unknown): Response => {
        const message = e instanceof Error ? e.message : String(e);
        // Chrome/Firefox report a missing or unregistered host with messages like these.
        const missing = /not found|not registered|no such native application|specified native messaging host/i.test(message);
        return {
          v: PROTOCOL_VERSION,
          id: req.id,
          ok: false,
          error: missing
            ? { code: 'HOST_NOT_INSTALLED', message: `The connection to ${SHORT_NAME} is not set up. Open ${SHORT_NAME}, then Settings, Browser, and register this browser.` }
            : { code: 'HOST_ERROR', message: `Could not reach ${SHORT_NAME}.` },
        };
      },
    );
    return await Promise.race([call, timeout]);
  } finally {
    clearTimeout(timer);
  }
}
