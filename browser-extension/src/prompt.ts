import type { PendingPrompt } from './background-core';
import type { Response } from './shared/protocol';
import { send } from './shared/ext';

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
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
    $('name').textContent = 'This request has expired.';
    $<HTMLButtonElement>('velocity').disabled = true;
    return;
  }
  const name = p.payload.filename?.split(/[\\/]/).pop() || new URL(p.payload.url).pathname.split('/').pop() || p.payload.url;
  $('name').textContent = name;
  $('meta').textContent = `${fmtSize(p.payload.fileSize)} · ${new URL(p.payload.url).host}`;
  $('velocity').addEventListener('click', () => {
    $<HTMLButtonElement>('velocity').disabled = true;
    $('error').textContent = '';
    void send<Response>({ type: 'prompt.decide', id, choice: 'velocity' }).then((r) => {
      if (!r.ok) {
        $('error').textContent = r.error.message;
        $<HTMLButtonElement>('velocity').disabled = false;
      }
    });
  });
  $('browser').addEventListener('click', () => void send({ type: 'prompt.decide', id, choice: 'browser' }));
}

void init();
