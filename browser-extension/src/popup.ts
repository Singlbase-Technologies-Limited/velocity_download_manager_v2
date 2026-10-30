import type { Response } from './shared/protocol';
import { ext, send } from './shared/ext';
import { sanitizeSettings, type InterceptMode } from './shared/settings';
import { SHORT_NAME } from './shared/branding';
import { $ } from './shared/dom';


async function refreshStatus() {
  const el = $('status', HTMLElement);
  el.textContent = 'Checking…';
  el.className = 'status';
  const r = await send<Response>({ type: 'status' });
  if (r.ok) {
    const engine = r.result.engineRunning === true;
    const accepts = r.result.acceptsRequests === true;
    el.textContent = !accepts ? `Connected. Browser requests are turned off in ${SHORT_NAME}.` : engine ? `Connected to ${SHORT_NAME} ${String(r.result.version)}` : 'Connected, but the download engine is not running.';
    el.className = `status ${accepts && engine ? 'ok' : 'warn'}`;
  } else {
    el.textContent = r.error.message;
    el.className = 'status err';
  }
}

async function init() {
  const s = sanitizeSettings((await ext.storage.local.get('settings')).settings);
  const mode = $('mode', HTMLSelectElement);
  mode.value = s.mode;
  mode.addEventListener('change', () => {
    void ext.storage.local.set({ settings: { ...s, mode: mode.value as InterceptMode } });
  });
  $('form', HTMLElement).addEventListener('submit', (e) => {
    e.preventDefault();
    const input = $('url', HTMLInputElement);
    const out = $('result', HTMLElement);
    out.textContent = 'Sending…';
    void send<Response>({ type: 'add', url: input.value }).then((r) => {
      out.textContent = r.ok ? `Sent to ${SHORT_NAME}.` : r.error.message;
      out.className = r.ok ? 'ok' : 'err';
      if (r.ok) input.value = '';
    });
  });
  $('options', HTMLElement).addEventListener('click', () => void ext.runtime.openOptionsPage());
  void refreshStatus();
}

void init();
