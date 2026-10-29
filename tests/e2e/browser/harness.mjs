// The real-browser tests' set-up (#64, #65), ported from V1's tests/e2e/harness.mjs: a throwaway
// data folder and home, a local file server, V2 started twice (the second time as a user who
// finished the setup and turned off "Ask before adding"), and checks that V2 took the link once
// and holds it to confirm its local address (#67), without connecting. V1's extension runs beside V2's, talking to a recording
// stand-in for V1's native host, to show each app gets only its own messages. Everything listens
// on 127.0.0.1 only. See docs/BROWSER_EXTENSION.md#tests.
import { execFileSync, spawn } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import http from 'node:http';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';

export const root = resolve(import.meta.dirname, '../../..');
export const branding = JSON.parse(readFileSync(join(root, 'branding.json'), 'utf8'));
const exe = process.platform === 'win32' ? '.exe' : '';
export const APP = process.env.VELOCITY_V2_APP ?? join(root, `target/debug/velocity-desktop-v2${exe}`);
export const HOST = process.env.VELOCITY_V2_HOST ?? join(root, `target/debug/velocity-native-host-v2${exe}`);
/** V2's extension, built by `pnpm build:extension`. */
export const extension = (target) => join(root, 'browser-extension/dist', target);
/** V1's built extension (V1 at e9d717a8), beside V2's checkout unless `V1_EXTENSION_DIR` says where. */
export const v1Extension = (target) =>
  process.env.V1_EXTENSION_DIR ? join(process.env.V1_EXTENSION_DIR, target) : resolve(root, '..', 'velocity-download-manager/browser-extension/dist', target);
/** V1's identity (V1 `branding.json` at e9d717a8). */
export const V1 = { nativeHostName: 'com.velocity.download_manager', firefoxExtensionId: 'velocity-download-manager@velocity.local', chromeExtensionId: 'eecopkgfiffjgmngfmjmdaejbkbbippo' };
export const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const PYTHON = process.platform === 'win32' ? 'python' : 'python3';

const results = [];
export const check = (name, ok, detail = '') => {
  results.push({ name, ok, detail });
  console.log(`${ok ? 'PASS' : 'FAIL'} ${name}${detail ? ` (${detail})` : ''}`);
};

export async function waitFor(fn, ms, label) {
  const end = Date.now() + ms;
  for (;;) {
    const v = await fn();
    if (v) return v;
    if (Date.now() > end) throw new Error(`timed out waiting for ${label}`);
    await sleep(250);
  }
}

/** A native host manifest for V2's host, or V1's stand-in, allowing `allowed` (origins or add-on IDs). */
export function hostManifest(name, path, firefox, allowed) {
  return JSON.stringify({ name, description: 'e2e', path, type: 'stdio', [firefox ? 'allowed_extensions' : 'allowed_origins']: allowed });
}

/**
 * A stand-in for V1's native host: it records who called and each message, and answers that V1 isn't running, so
 * V1's extension lets the browser carry on. Node runs it; on Windows through a .cmd file, which browsers can start.
 */
function v1Stub(dir) {
  const log = join(dir, 'v1-host.log');
  const script = join(dir, 'v1-host.mjs');
  writeFileSync(
    script,
    `import { appendFileSync } from 'node:fs';
const log = ${JSON.stringify(log)};
appendFileSync(log, JSON.stringify({ args: process.argv.slice(2) }) + '\\n');
let buf = Buffer.alloc(0);
process.stdin.on('data', (chunk) => {
  buf = Buffer.concat([buf, chunk]);
  while (buf.length >= 4 && buf.length >= 4 + buf.readUInt32LE(0)) {
    const n = buf.readUInt32LE(0);
    const message = JSON.parse(buf.subarray(4, 4 + n).toString('utf8'));
    buf = buf.subarray(4 + n);
    appendFileSync(log, JSON.stringify({ message }) + '\\n');
    const body = Buffer.from(JSON.stringify({ v: 1, id: message.id, ok: false, error: { code: 'DESKTOP_NOT_RUNNING', message: 'V1 stand-in' } }));
    const len = Buffer.alloc(4);
    len.writeUInt32LE(body.length, 0);
    process.stdout.write(Buffer.concat([len, body]));
  }
});
`,
  );
  let path;
  if (process.platform === 'win32') {
    path = join(dir, 'v1-host.cmd');
    writeFileSync(path, `@echo off\r\n"${process.execPath}" "${script}" %*\r\n`);
  } else {
    path = join(dir, 'v1-host.sh');
    writeFileSync(path, `#!/bin/sh\nexec "${process.execPath}" "${script}" "$@"\n`);
    chmodSync(path, 0o755);
  }
  const entries = () => (existsSync(log) ? readFileSync(log, 'utf8').trim().split('\n').filter(Boolean).map((l) => JSON.parse(l)) : []);
  return { path, entries };
}

export async function setup(prefix, required) {
  for (const p of [APP, HOST, ...required]) if (!existsSync(p)) throw new Error(`missing ${p}; build it first (docs/BROWSER_EXTENSION.md#tests)`);
  const D = mkdtempSync(join(tmpdir(), prefix));
  const data = join(D, 'data');
  const downloads = join(D, 'downloads');
  mkdirSync(downloads, { recursive: true });
  const home = join(D, 'home');
  mkdirSync(home, { recursive: true });
  const env = { ...process.env, VELOCITY_V2_DATA_DIR: data, HOME: home };

  const payload = randomBytes(3 * 1024 * 1024);
  // Each request for the file, with its User-Agent: a browser always sends one, and V2's engine sends none.
  const fileRequests = [];
  const server = http.createServer((req, res) => {
    if (req.url === '/files/e2e-archive.zip') fileRequests.push(req.headers['user-agent'] ?? '');
    if (req.url === '/') {
      res.writeHead(200, { 'content-type': 'text/html' });
      res.end('<!doctype html><a id="dl" href="/files/e2e-archive.zip">download</a>');
    } else if (req.url === '/files/e2e-archive.zip') {
      res.writeHead(200, { 'content-type': 'application/zip', 'content-length': payload.length, 'content-disposition': 'attachment; filename="e2e-archive.zip"' });
      // A little throttled, so the browser can't finish before the extension pauses it.
      let off = 0;
      const tick = () => {
        if (off >= payload.length) return res.end();
        res.write(payload.subarray(off, off + 256 * 1024));
        off += 256 * 1024;
        setTimeout(tick, 50);
      };
      tick();
    } else {
      res.writeHead(404).end();
    }
  });
  await new Promise((r) => server.listen(0, '127.0.0.1', r));
  const base = `http://127.0.0.1:${server.address().port}`;
  const dbPath = join(data, 'velocity.sqlite3');
  const tokenPath = join(data, 'ipc.token');

  let app;
  const startApp = () => {
    const p = spawn(APP, [], { env, stdio: ['ignore', 'ignore', 'pipe'] });
    p.stderr.on('data', () => undefined);
    return p;
  };
  const stopApp = async (p) => {
    p.kill();
    await new Promise((r) => (p.exitCode !== null ? r() : p.once('exit', r)));
  };

  /** The first start makes the database; then the settings a user who finished the setup and turned off "Ask before adding" has. */
  const startPreseeded = async () => {
    app = startApp();
    await waitFor(() => existsSync(tokenPath) && existsSync(dbPath), 30000, 'the first app start');
    await stopApp(app);
    rmSync(tokenPath, { force: true });
    execFileSync(PYTHON, [
      '-I',
      '-c',
      `
import json, sqlite3, sys
db = sqlite3.connect(sys.argv[1])
row = db.execute("select document from settings where id = 1").fetchone()
d = json.loads(row[0]) if row else {}
d.setdefault('downloads', {})['defaultDirectory'] = sys.argv[2]
d['onboarding'] = {'completed': True}
d['browser'] = {'acceptRequests': True, 'confirmBeforeAdding': False}
db.execute("insert into settings (id, document, updated_at_ms) values (1, ?, 0) on conflict (id) do update set document = excluded.document", (json.dumps(d),))
db.commit()
`,
      dbPath,
      downloads,
    ]);
    app = startApp();
    await waitFor(() => existsSync(tokenPath), 30000, 'the second app start');
    await sleep(1500);
  };

  const find = (dir, name) => {
    for (const e of readdirSync(dir, { withFileTypes: true })) {
      const p = join(dir, e.name);
      if (e.isFile() && e.name === name) return p;
      if (e.isDirectory()) {
        const found = find(p, name);
        if (found) return found;
      }
    }
    return null;
  };

  const query = (sql) => execFileSync(PYTHON, ['-I', '-c', 'import sqlite3,sys; print(sqlite3.connect(sys.argv[1]).execute(sys.argv[2]).fetchone()[0])', dbPath, sql]).toString().trim();
  /**
   * V2 took the link once and holds it for the user: the test page is on 127.0.0.1, and a link from the browser to
   * a local address waits for "Download anyway" in the details panel (#67, ADR-0006). This test has no window to
   * click it in, so it checks V2 stopped before connecting: no request for the file came from V2, and no file was
   * written. The download itself is the engine tests' to check.
   */
  const checkV2Held = async () => {
    const records = () => query("select count(*) || ' ' || group_concat(json_extract(error_json, '$.confirm.kind') || '@' || json_extract(error_json, '$.confirm.address')) from downloads where source_url like '%e2e-archive.zip'");
    const held = await waitFor(() => (records() === '1 localAddress@127.0.0.1' ? records() : null), 60000, 'the link held for confirmation').catch(() => records());
    check('V2 recorded the link once, held to confirm its local address', held === '1 localAddress@127.0.0.1', held);
    const fromV2 = fileRequests.filter((ua) => !ua.includes('Mozilla')).length;
    check('V2 asked the server for nothing before the user confirmed', fromV2 === 0, `${fromV2} request(s) without a browser's User-Agent`);
    const written = find(downloads, 'e2e-archive.zip');
    check('V2 wrote no file', written === null, written ?? '');
  };

  const stub = v1Stub(D);
  /** V1's extension reached only V1's host, and V2's never did. */
  const checkV1Isolated = (v1Caller) => {
    const entries = stub.entries();
    const callers = entries.filter((e) => e.args).map((e) => e.args.join(' '));
    const adds = entries.filter((e) => e.message?.type === 'download.add');
    check("V1's extension sent the link to V1's host", adds.some((e) => String(e.message.payload?.url).endsWith('e2e-archive.zip')), `${adds.length} add message(s)`);
    check("only V1's extension started V1's host", callers.length > 0 && callers.every((c) => c.includes(v1Caller)), callers.join(' | '));
  };

  const teardown = async () => {
    if (app && app.exitCode === null) await stopApp(app);
    server.close();
    // On Windows a process that just exited can hold a file a moment longer; what is left is said, not hidden.
    if (!process.env.KEEP_E2E) {
      try {
        rmSync(D, { recursive: true, force: true, maxRetries: 5, retryDelay: 500 });
      } catch (e) {
        console.log(`note: ${D} was left behind (${String(e?.message ?? e)})`);
      }
    }
  };

  return { D, env, downloads, base, stub, startPreseeded, checkV2Held, checkV1Isolated, teardown };
}

export function finish() {
  const failed = results.filter((r) => !r.ok).length;
  console.log(`\n${results.length - failed}/${results.length} checks passed`);
  process.exit(failed ? 1 : 0);
}
