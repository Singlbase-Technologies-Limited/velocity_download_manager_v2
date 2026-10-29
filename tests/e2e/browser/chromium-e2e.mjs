// Real Chromium (Playwright's) with V2's extension and V1's side by side (#64, #65): a link clicked on a page is
// intercepted by V2's extension, goes through V2's native host and the authenticated local connection into the
// running V2, which records it once and holds it to confirm its local address (#67); the browser's own copy is
// cancelled and erased. V1's extension, also installed, reaches only V1's host (a recording stand-in).
//
// Build first: `cargo build -p velocity-desktop-v2 -p velocity-transfer -p velocity-torrent -p velocity-native-host`
// and `pnpm build:extension`; V1's extension built at ../velocity-download-manager (or V1_EXTENSION_DIR). Then
// `pnpm test:e2e:chromium` (on Linux under a display, such as WSLg or `xvfb-run -a`). See docs/BROWSER_EXTENSION.md.
import { chromium } from '@playwright/test';
import { execFileSync } from 'node:child_process';
import { mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { HOST, V1, branding, check, extension, finish, hostManifest, setup, v1Extension, waitFor } from './harness.mjs';

const EXT = extension('chrome');
const V1_EXT = v1Extension('chrome');
const w = await setup('v2-chromium-e2e-', [join(EXT, 'manifest.json'), join(V1_EXT, 'manifest.json')]);
const profile = join(w.D, 'chromium-profile');
const v2Origin = `chrome-extension://${branding.chromeExtensionIds[0]}/`;
const v1Origin = `chrome-extension://${V1.chromeExtensionId}/`;

/**
 * Where this Chromium reads native hosts. On Linux, `<profile>/NativeMessagingHosts`. On Windows, only the registry
 * (HKCU\Software\Chromium\NativeMessagingHosts): the keys are V2's development host's and the stand-in's, removed
 * afterwards; a key already there (a real V1 or V2 registration) is put back as it was.
 */
const registered = [];
function register(name, manifest) {
  if (process.platform !== 'win32') {
    mkdirSync(join(profile, 'NativeMessagingHosts'), { recursive: true });
    writeFileSync(join(profile, 'NativeMessagingHosts', `${name}.json`), manifest);
    return;
  }
  const file = join(w.D, `${name}.json`);
  writeFileSync(file, manifest);
  const key = `HKCU\\Software\\Chromium\\NativeMessagingHosts\\${name}`;
  let before = null;
  try {
    before = execFileSync('reg', ['query', key, '/ve'], { encoding: 'utf8' }).match(/REG_SZ\s+(.*)/)?.[1]?.trim() ?? null;
  } catch {
    before = null;
  }
  execFileSync('reg', ['add', key, '/ve', '/t', 'REG_SZ', '/d', file, '/f'], { stdio: 'ignore' });
  registered.push({ key, before });
}
function unregister() {
  for (const { key, before } of registered) {
    if (before === null) execFileSync('reg', ['delete', key, '/f'], { stdio: 'ignore' });
    else execFileSync('reg', ['add', key, '/ve', '/t', 'REG_SZ', '/d', before, '/f'], { stdio: 'ignore' });
  }
}

let context;
try {
  await w.startPreseeded();
  register(branding.nativeHostName, hostManifest(branding.nativeHostName, HOST, false, [v2Origin]));
  register(V1.nativeHostName, hostManifest(V1.nativeHostName, w.stub.path, false, [v1Origin]));

  context = await chromium.launchPersistentContext(profile, {
    headless: false,
    ...(process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {}),
    env: w.env,
    acceptDownloads: true,
    args: [`--disable-extensions-except=${EXT},${V1_EXT}`, `--load-extension=${EXT},${V1_EXT}`, '--no-first-run'],
  });
  const workers = await waitFor(() => (context.serviceWorkers().length >= 2 ? context.serviceWorkers() : null), 15000, 'both extensions');
  const ids = workers.map((sw) => new URL(sw.url()).host);
  check("V2's extension ID is the one its key pins", ids.includes(branding.chromeExtensionIds[0]), ids.join(', '));
  check("V1's extension is installed beside it", ids.includes(V1.chromeExtensionId), ids.join(', '));

  const popup = await context.newPage();
  await popup.goto(`${v2Origin}popup.html`);
  const status = await waitFor(async () => {
    const t = (await popup.textContent('#status')) ?? '';
    return t && !t.startsWith('Checking') ? t : null;
  }, 15000, 'the popup status');
  check("V2's popup reaches V2 through V2's native host", status.startsWith('Connected to Velocity V2'), status);

  const page = await context.newPage();
  await page.goto(`${w.base}/`);
  await page.click('#dl');

  await w.checkV2Held();

  const browserItems = await waitFor(async () => {
    const items = await popup.evaluate(() => chrome.downloads.search({}));
    return items.every((i) => i.state !== 'in_progress') ? items : null;
  }, 15000, 'the browser download to be cancelled');
  const left = browserItems.filter((i) => i.state === 'complete' && i.filename.endsWith('e2e-archive.zip'));
  check("the browser's copy was cancelled", left.length === 0, `${left.length} complete copy(ies)`);

  w.checkV1Isolated(v1Origin);
} catch (e) {
  check('Chromium end-to-end run', false, String(e?.message ?? e));
} finally {
  await context?.close().catch(() => undefined);
  try {
    unregister();
  } catch (e) {
    check('the test registry keys were removed', false, String(e?.message ?? e));
  }
  await w.teardown();
}
finish();
