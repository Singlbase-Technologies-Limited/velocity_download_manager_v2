// Real Firefox, driven through geckodriver's W3C WebDriver API (no npm package), with V2's add-on and V1's side by
// side (#64, #65): ported from V1's tests/e2e/firefox-extension-e2e.mjs. A link clicked on a page goes through V2's
// add-on and native host into the running V2, which records it once and holds it to confirm its local address (#67);
// Firefox's own copy is cancelled. V1's add-on reaches only V1's host (a recording stand-in).
//
// Linux only (PO decision on #64): Firefox reads ~/.mozilla/native-messaging-hosts, and HOME is the throwaway one.
// Build as for the Chromium test, then `pnpm test:e2e:firefox` under a display. Needs `firefox`, `geckodriver` and
// `zip` (or FIREFOX_PATH and GECKODRIVER_PATH). See docs/BROWSER_EXTENSION.md.
import { execFileSync, spawn } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { mkdirSync, readdirSync, statSync, writeFileSync } from 'node:fs';
import net from 'node:net';
import { join } from 'node:path';
import { HOST, V1, branding, check, extension, finish, hostManifest, setup, sleep, v1Extension, waitFor } from './harness.mjs';

const EXT = extension('firefox');
const V1_EXT = v1Extension('firefox');
const FIREFOX = process.env.FIREFOX_PATH ?? 'firefox';
const GECKODRIVER = process.env.GECKODRIVER_PATH ?? 'geckodriver';
const w = await setup('v2-firefox-e2e-', [join(EXT, 'manifest.json'), join(V1_EXT, 'manifest.json')]);
const browserDownloads = join(w.D, 'firefox-downloads');
mkdirSync(browserDownloads, { recursive: true });

const freePort = () =>
  new Promise((r) => {
    const s = net.createServer().listen(0, '127.0.0.1', () => {
      const { port } = s.address();
      s.close(() => r(port));
    });
  });

let driver;
let wd;
let session;
async function cmd(method, path, body) {
  const res = await fetch(`${wd}${path}`, { method, headers: { 'content-type': 'application/json' }, body: body === undefined ? undefined : JSON.stringify(body) });
  const json = await res.json();
  if (!res.ok) throw new Error(`${method} ${path}: ${json.value?.error ?? res.status} ${json.value?.message ?? ''}`);
  return json.value;
}
const s = (path) => `/session/${session}${path}`;

try {
  if (process.platform === 'win32') throw new Error('the Firefox test runs on Linux only (docs/BROWSER_EXTENSION.md#tests)');
  await w.startPreseeded();

  const hosts = join(w.env.HOME, '.mozilla', 'native-messaging-hosts');
  mkdirSync(hosts, { recursive: true });
  writeFileSync(join(hosts, `${branding.nativeHostName}.json`), hostManifest(branding.nativeHostName, HOST, true, [branding.firefoxExtensionId]));
  writeFileSync(join(hosts, `${V1.nativeHostName}.json`), hostManifest(V1.nativeHostName, w.stub.path, true, [V1.firefoxExtensionId]));

  // Each add-on is packed from its built folder, as it would be uploaded.
  const pack = (dir, name) => {
    const xpi = join(w.D, name);
    execFileSync('zip', ['-q', '-r', xpi, '.'], { cwd: dir });
    return xpi;
  };
  const v2Xpi = pack(EXT, 'velocity-v2.xpi');
  const v1Xpi = pack(V1_EXT, 'velocity-v1.xpi');

  const port = await freePort();
  // --allow-system-access lets the test open the add-on's page and read Firefox's download list from the browser chrome.
  driver = spawn(GECKODRIVER, ['--port', String(port), '--host', '127.0.0.1', '--allow-system-access'], { env: w.env, stdio: ['ignore', 'ignore', 'pipe'] });
  driver.stderr.on('data', () => undefined);
  wd = `http://127.0.0.1:${port}`;
  await waitFor(() => fetch(`${wd}/status`).then((r) => r.ok).catch(() => false), 15000, 'geckodriver');

  // A fixed moz-extension UUID lets the test open V2's popup directly.
  const uuid = randomUUID();
  const created = await cmd('POST', '/session', {
    capabilities: {
      alwaysMatch: {
        browserName: 'firefox',
        'moz:firefoxOptions': {
          ...(process.env.FIREFOX_PATH ? { binary: FIREFOX } : {}),
          env: { HOME: w.env.HOME },
          prefs: {
            'extensions.webextensions.uuids': JSON.stringify({ [branding.firefoxExtensionId]: uuid }),
            'browser.download.dir': browserDownloads,
            'browser.download.folderList': 2,
            'browser.download.useDownloadDir': true,
            'browser.download.always_ask_before_handling_new_types': false,
            'browser.download.alwaysOpenPanel': false,
          },
        },
      },
    },
  });
  session = created.sessionId;
  check('Firefox started through geckodriver', Boolean(session), created.capabilities?.browserVersion ?? '');

  const id = await cmd('POST', s('/moz/addon/install'), { path: v2Xpi, temporary: true });
  check("V2's add-on installs with its pinned ID", id === branding.firefoxExtensionId, String(id));
  const v1Id = await cmd('POST', s('/moz/addon/install'), { path: v1Xpi, temporary: true });
  check("V1's add-on is installed beside it", v1Id === V1.firefoxExtensionId, String(v1Id));

  // WebDriver may not navigate to moz-extension: pages, so the tab is opened from the browser chrome, as a click on
  // the toolbar button would.
  const before = await cmd('GET', s('/window/handles'));
  await cmd('POST', s('/moz/context'), { context: 'chrome' });
  await cmd('POST', s('/execute/sync'), {
    script: 'gBrowser.selectedTab = gBrowser.addTab(arguments[0], { triggeringPrincipal: Services.scriptSecurityManager.getSystemPrincipal() });',
    args: [`moz-extension://${uuid}/popup.html`],
  });
  await cmd('POST', s('/moz/context'), { context: 'content' });
  const popupHandle = await waitFor(async () => (await cmd('GET', s('/window/handles'))).find((h) => !before.includes(h)), 10000, 'the add-on tab');
  await cmd('POST', s('/window'), { handle: popupHandle });
  const status = await waitFor(async () => {
    const el = await cmd('POST', s('/element'), { using: 'css selector', value: '#status' });
    const t = await cmd('GET', s(`/element/${Object.values(el)[0]}/text`));
    return t && !t.startsWith('Checking') ? t : null;
  }, 15000, 'the popup status');
  check("V2's popup reaches V2 through V2's native host", status.startsWith('Connected to Velocity V2'), status);

  const tab = await cmd('POST', s('/window/new'), { type: 'tab' });
  await cmd('POST', s('/window'), { handle: tab.handle });
  await cmd('POST', s('/url'), { url: `${w.base}/` });
  const link = await cmd('POST', s('/element'), { using: 'css selector', value: '#dl' });
  await cmd('POST', s(`/element/${Object.values(link)[0]}/click`), {});

  await w.checkV2Held();

  // Firefox's own copy: no complete file in its download folder.
  await sleep(1000);
  const leftovers = readdirSync(browserDownloads).filter((f) => f.startsWith('e2e-archive') && statSync(join(browserDownloads, f)).size > 0);
  check('Firefox left no copy of the file in its download folder', leftovers.length === 0, leftovers.join(', '));

  w.checkV1Isolated(V1.firefoxExtensionId);
} catch (e) {
  check('Firefox end-to-end run', false, String(e?.message ?? e));
} finally {
  if (session) await cmd('DELETE', s('')).catch(() => undefined);
  if (driver && driver.exitCode === null) driver.kill('SIGTERM');
  await w.teardown();
}
finish();
