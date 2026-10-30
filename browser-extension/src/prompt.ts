import type { PendingPrompt } from './background-core';
import type { Response } from './shared/protocol';
import { send } from './shared/ext';
import { $ } from './shared/dom';

const id = new URLSearchParams(location.search).get('id') ?? '';

function fmtSize(n: number | undefined): string {
  if (!n) return 'Unknown size';
  const u = ['B', 'KB', 'MB', 'GB', 'TB'];
  let v = n;
  let i = 0;
  while (v >= 1024 && i < u.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v.toFixed(i === 0 ? 0 : 1)} ${u[i] ?? 'B'}`;
}

async function init() {
  const p = await send<PendingPrompt | null>({ type: 'prompt.get', id });
  if (!p) {
    $('name', HTMLElement).textContent = 'This request has expired.';
    $('velocity', HTMLButtonElement).disabled = true;
    return;
  }
  const name = p.payload.filename?.split(/[\\/]/).pop() || new URL(p.payload.url).pathname.split('/').pop() || p.payload.url;
  $('name', HTMLElement).textContent = name;
  $('meta', HTMLElement).textContent = `${fmtSize(p.payload.fileSize)} · ${new URL(p.payload.url).host}`;
  $('velocity', HTMLElement).addEventListener('click', () => {
    $('velocity', HTMLButtonElement).disabled = true;
    $('error', HTMLElement).textContent = '';
    void send<Response>({ type: 'prompt.decide', id, choice: 'velocity' }).then((r) => {
      if (!r.ok) {
        $('error', HTMLElement).textContent = r.error.message;
        $('velocity', HTMLButtonElement).disabled = false;
      }
    });
  });
  $('browser', HTMLElement).addEventListener('click', () => void send({ type: 'prompt.decide', id, choice: 'browser' }));
}

void init();
