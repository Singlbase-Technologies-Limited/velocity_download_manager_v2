// The layout checks in a real Chromium (L4, V1's tests/e2e/layout): the window runs against the mocked IPC in
// harness.tsx, served by Vite's dev server, so no desktop shell or engine is needed. Usage:
//   pnpm test:e2e:layout [-- --shots <dir>]
// The browser comes from `pnpm exec playwright install chromium` (docs/TESTING.md), or CHROMIUM_PATH.
import { chromium } from '@playwright/test';
import { mkdirSync, realpathSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { createServer } from 'vite';

const root = resolve(import.meta.dirname, '../../..');
const args = process.argv.slice(2);
const opt = (name, fallback) => {
  const i = args.indexOf(name);
  return i >= 0 && args[i + 1] ? args[i + 1] : fallback;
};
const SHOTS = opt('--shots', null);
const PORT = Number(opt('--port', '15873'));
const BASE = `http://127.0.0.1:${String(PORT)}/tests/e2e/layout/harness.html`;
const failedRequests = [];

const results = [];
const check = (name, ok, detail = '') => {
  results.push({ name, ok });
  console.log(`${ok ? 'PASS' : 'FAIL'} ${name}${detail ? ` (${detail})` : ''}`);
};

// Vite's dev server in this process. node_modules may be a link (in a worktree), so its real place is allowed too.
const vite = await createServer({
  root,
  configFile: join(root, 'vite.config.ts'),
  logLevel: 'warn',
  server: { port: PORT, strictPort: true, host: '127.0.0.1', fs: { allow: [root, realpathSync(join(root, 'node_modules'))] } },
});
await vite.listen();

const executablePath = process.env.CHROMIUM_PATH;
const browser = await chromium.launch(executablePath ? { executablePath } : {});

async function open(page, query = '') {
  await page.goto(`${BASE}?${query}`);
  await page.getByRole('grid', { name: 'Downloads' }).waitFor();
  await page.getByText('archive.zip').first().waitFor();
}

async function shot(page, name) {
  if (!SHOTS) return;
  mkdirSync(SHOTS, { recursive: true });
  await page.waitForTimeout(150);
  await page.screenshot({ path: join(SHOTS, `${name}.png`) });
}

const row = (page, text) => page.getByRole('grid', { name: 'Downloads' }).getByRole('row').filter({ hasText: text }).first();

async function newPage(width, height) {
  const ctx = await browser.newContext({ viewport: { width, height }, deviceScaleFactor: 1 });
  const page = await ctx.newPage();
  page.on('response', (r) => {
    if (r.status() >= 400 && !r.url().endsWith('/favicon.ico')) failedRequests.push(`${String(r.status())} ${r.url()}`);
  });
  page.on('pageerror', (e) => { failedRequests.push(`page error: ${e.message}`); });
  return { ctx, page };
}

async function screenshots() {
  for (const theme of ['light', 'dark']) {
    for (const [label, w, h] of [
      ['laptop-1366x768', 1366, 768],
      ['wide-1920x1080', 1920, 1080],
      ['min-960x600', 960, 600],
    ]) {
      const { ctx, page } = await newPage(w, h);
      await open(page, `fresh=1&theme=${theme}`);
      await row(page, 'archive.zip').click();
      await shot(page, `${label}-${theme}-list`);
      await ctx.close();
    }
  }
}

try {
  const { runChecks } = await import('./layout-checks.mjs');
  await runChecks({ open, shot, row, newPage, check, BASE });
  await screenshots();
  check('harness: every asset loaded, and no page error', failedRequests.length === 0, failedRequests.slice(0, 3).join(', '));
} finally {
  await browser.close();
  await vite.close();
}

const failed = results.filter((r) => !r.ok);
console.log(`\n${String(results.length - failed.length)} passed, ${String(failed.length)} failed`);
if (failed.length > 0) process.exit(1);
