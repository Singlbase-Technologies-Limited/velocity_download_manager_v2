#!/usr/bin/env node
// The field test's session tool (#262, tests/field/README.md): starts the installed app with
// WebView2's debugging port on 127.0.0.1, drives its window as a user does (Playwright over CDP),
// and watches the downloads. Run by hand only, never in CI.
//
//   pnpm field start                      start the installed app with the port
//   pnpm field onboard [folder]           finish the first-run setup
//   pnpm field add <link> [--connections N] [--folder DIR]
//   pnpm field pause|resume|cancel|retry|remove <name>
//   pnpm field click <role> <name>        any other control, by its accessible role and name
//   pnpm field watch [--every S] [--for S] [--until done]
//   pnpm field status | log [--last N] | export | shot <file>
//   pnpm field quit                       quit as the tray's Quit does
//   pnpm field kill [app|engine|torrent]  end a process at once, as a crash does
//
// Everything a session writes goes to its folder (--out, default %TEMP%\velocity-field\<date>),
// outside the repository, with every link cut to its host (field-lib.mjs).

import { spawn, spawnSync } from 'node:child_process';
import { appendFileSync, existsSync, mkdirSync, statSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { chromium } from '@playwright/test';
import { findings, hideLinks, sample } from './field-lib.mjs';

const PROCESSES = { app: 'velocity-desktop-v2', engine: 'velocity-transfer', torrent: 'velocity-torrent' };
const TERMINAL = new Set(['completed', 'failed', 'removed', 'seeding']);

function options(args) {
  const named = {};
  const rest = [];
  for (let i = 0; i < args.length; i++) {
    if (args[i].startsWith('--')) named[args[i].slice(2)] = args[++i];
    else rest.push(args[i]);
  }
  return { named, rest };
}

const [command, ...args] = process.argv.slice(2);
const { named, rest } = options(args);
const port = Number(named.port ?? 9333);
const out = named.out ?? join(tmpdir(), 'velocity-field', new Date().toISOString().slice(0, 10));
const exe = named.exe ?? join(process.env.LOCALAPPDATA ?? '', 'Velocity Download Manager V2', `${PROCESSES.app}.exe`);

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const mib = (bytes) => (bytes == null ? '?' : `${(bytes / 1048576).toFixed(1)} MiB`);
const clock = () => new Date().toTimeString().slice(0, 8);

async function portAnswers() {
  try {
    return (await fetch(`http://127.0.0.1:${port}/json/version`)).ok;
  } catch {
    return false;
  }
}

function running(name) {
  const r = spawnSync('tasklist', ['/FI', `IMAGENAME eq ${name}.exe`, '/FO', 'CSV', '/NH'], { encoding: 'utf8' });
  return r.stdout.includes(`"${name}.exe"`);
}

async function start() {
  if (await portAnswers()) return console.log(`The app already answers on 127.0.0.1:${port}.`);
  // The app runs one instance: a second start only shows the first, without the port.
  if (running(PROCESSES.app)) throw new Error('The app is running without the debugging port. Quit it from the tray first.');
  if (!existsSync(exe)) throw new Error(`${exe} isn't installed (see tests/field/README.md).`);
  const env = { ...process.env, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port} --remote-debugging-address=127.0.0.1` };
  spawn(exe, [], { env, detached: true, stdio: 'ignore' }).unref();
  for (let i = 0; i < 60; i++) {
    if (await portAnswers()) return console.log(`Started; the window answers on 127.0.0.1:${port}.`);
    await sleep(500);
  }
  throw new Error('The app started but its window never answered on the port.');
}

/** Runs `f` with the app's page, then lets go of it; the app keeps running. */
async function withPage(f) {
  const browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`);
  try {
    const page = browser.contexts()[0]?.pages()[0];
    if (!page) throw new Error('The app has no window to drive.');
    return await f(page);
  } finally {
    await browser.close();
  }
}

/** One of the app's typed commands, read-only ones only: what the window itself would ask. */
const invoke = (page, cmd, a) => page.evaluate(([c, x]) => window.__TAURI_INTERNALS__.invoke(c, x), [cmd, a]);

function lengthOf(path) {
  try {
    return statSync(path).size;
  } catch {
    return null;
  }
}

function samples(snapshot) {
  return snapshot.downloads.map((d) => {
    const file = d.fileName ? join(d.directory, d.fileName) : null;
    return sample(d, { partBytes: file && lengthOf(`${file}.vdmpart`), fileBytes: file && lengthOf(file) });
  });
}

/** The app's processes, and their open TCP connections by remote address. */
function processes() {
  const script = `$ps = Get-Process -Name ${Object.values(PROCESSES).join(',')} -ErrorAction SilentlyContinue
$ids = @($ps | ForEach-Object { $_.Id })
$tcp = @(Get-NetTCPConnection -State Established -ErrorAction SilentlyContinue | Where-Object { $ids -contains $_.OwningProcess -and $_.RemoteAddress -ne '127.0.0.1' -and $_.RemoteAddress -ne '::1' } | ForEach-Object { @{ pid = $_.OwningProcess; remote = "$($_.RemoteAddress):$($_.RemotePort)" } })
@{ procs = @($ps | ForEach-Object { @{ name = $_.ProcessName; pid = $_.Id; mib = [math]::Round($_.WorkingSet64 / 1MB, 1); cpuSeconds = $_.CPU; handles = $_.HandleCount } }); tcp = $tcp } | ConvertTo-Json -Depth 4 -Compress`;
  const r = spawnSync('powershell', ['-NoProfile', '-NonInteractive', '-Command', script], { encoding: 'utf8' });
  if (r.status !== 0) return { error: r.stderr.trim() };
  const value = JSON.parse(r.stdout || '{}');
  const byProcess = {};
  for (const c of value.tcp ?? []) {
    const name = value.procs.find((p) => p.pid === c.pid)?.name ?? c.pid;
    (byProcess[name] ??= {})[c.remote] = (byProcess[name][c.remote] ?? 0) + 1;
  }
  return { procs: value.procs ?? [], connections: byProcess };
}

function line(s) {
  const pct = s.total ? `${((100 * s.done) / s.total).toFixed(1)}%` : mib(s.done);
  const conns = s.requested ? `${s.connections}/${s.requested} conns${s.limitedBy ? ` (${s.limitedBy})` : ''}` : `${s.connections} conns`;
  const part = s.partBytes != null ? ` part ${mib(s.partBytes)}` : '';
  return `${s.id} ${s.name ?? '(no name yet)'} ${s.status} ${pct} ${mib(s.speed)}/s ${conns}${part}${s.error ? ` | ${s.error}` : ''}`;
}

async function watch() {
  mkdirSync(out, { recursive: true });
  const every = Number(named.every ?? 5) * 1000;
  const until = Date.now() + Number(named.for ?? 600) * 1000;
  const last = new Map();
  const changedAt = new Map();
  console.log(`Watching every ${every / 1000} s; samples in ${out}`);
  while (Date.now() < until) {
    const now = Date.now();
    const snapshot = await withPage((page) => invoke(page, 'get_snapshot'));
    const current = samples(snapshot);
    const system = processes();
    appendFileSync(join(out, 'samples.jsonl'), `${JSON.stringify({ at: now, downloads: current, ...system })}\n`);
    for (const s of current) {
      const before = last.get(s.id);
      if (!before || before.done !== s.done) changedAt.set(s.id, now);
      console.log(`${clock()} ${line(s)}`);
      for (const f of before ? findings(before, s, now - changedAt.get(s.id)) : []) {
        console.log(`${clock()} FINDING ${f}`);
        appendFileSync(join(out, 'findings.jsonl'), `${JSON.stringify({ at: now, finding: f, sample: s })}\n`);
        await withPage((page) => page.screenshot({ path: join(out, `finding-${now}-${s.id}.png`) }));
      }
      last.set(s.id, s);
    }
    const engine = system.connections?.[PROCESSES.engine];
    if (engine) console.log(`${clock()} engine connections: ${JSON.stringify(engine)}`);
    if (named.until === 'done' && current.length > 0 && current.every((s) => TERMINAL.has(s.status))) break;
    await sleep(Math.max(0, every - (Date.now() - now)));
  }
}

async function row(page, name) {
  const r = page.getByRole('row').filter({ hasText: name });
  if ((await r.count()) === 0) throw new Error(`No download named ${name} in the list.`);
  await r.first().click();
}

/** Says what a confirmation that an action opened asks, and leaves it for `click` to answer. */
async function dialog(page) {
  await sleep(500);
  const open = page.getByRole('alertdialog').or(page.getByRole('dialog')).first();
  if (await open.isVisible()) console.log(`${clock()} the app asks: ${(await open.innerText()).replace(/\n+/g, ' | ')}`);
}

const ACTIONS = { pause: 'Pause selected', resume: 'Resume selected', cancel: 'Cancel selected', retry: 'Retry selected', remove: 'Remove selected from list' };

async function main() {
  switch (command) {
    case 'start':
      return start();
    case 'onboard':
      return withPage(async (page) => {
        if (rest[0]) await page.getByRole('textbox').first().fill(rest[0]);
        await page.getByRole('button', { name: 'Get started' }).click();
        console.log('Setup finished.');
      });
    case 'add':
      return withPage(async (page) => {
        await page.getByRole('button', { name: /^Add download/ }).click();
        await page.getByRole('textbox', { name: 'Links' }).fill(rest[0]);
        if (named.folder) await page.getByRole('textbox', { name: 'Save to' }).fill(named.folder);
        if (named.connections) {
          await page.getByRole('button', { name: 'Advanced options' }).click();
          await page.getByRole('spinbutton', { name: 'Connections' }).fill(named.connections);
        }
        await page.getByRole('button', { name: 'Start download' }).click();
        await sleep(1500);
        const done = page.getByRole('button', { name: 'Done' });
        if (await done.isVisible()) await done.click();
        mkdirSync(out, { recursive: true });
        const shot = join(out, `added-${Date.now()}.png`);
        await page.screenshot({ path: shot });
        console.log(`Added a link to ${hideLinks(rest[0])}; screenshot ${shot}`);
      });
    case 'pause':
    case 'resume':
    case 'cancel':
    case 'retry':
    case 'remove':
      return withPage(async (page) => {
        await row(page, rest[0]);
        await page.getByRole('button', { name: ACTIONS[command] }).click();
        console.log(`${clock()} ${command}: ${rest[0]}`);
        await dialog(page);
      });
    case 'click':
      return withPage(async (page) => {
        await page.getByRole(rest[0], { name: rest[1] }).first().click();
        console.log(`${clock()} clicked ${rest[0]} "${rest[1]}"`);
        await dialog(page);
      });
    case 'watch':
      return watch();
    case 'status':
      return withPage(async (page) => {
        for (const s of samples(await invoke(page, 'get_snapshot'))) console.log(line(s));
        console.log(JSON.stringify(processes()));
      });
    case 'log':
      return withPage(async (page) => {
        const report = await invoke(page, 'get_diagnostics');
        const lines = report.appLog.slice(-Number(named.last ?? 50));
        for (const l of lines) console.log(`${new Date(l.atMs).toTimeString().slice(0, 8)} ${l.level.padEnd(5)} ${hideLinks(l.message)}`);
      });
    case 'export':
      return withPage(async (page) => {
        mkdirSync(out, { recursive: true });
        const path = join(out, `diagnostics-${Date.now()}.json`);
        writeFileSync(path, hideLinks(JSON.stringify(await invoke(page, 'get_diagnostics'), null, 2)));
        console.log(`Diagnostics written to ${path}`);
      });
    case 'shot':
      return withPage(async (page) => {
        await page.screenshot({ path: rest[0] });
        console.log(`Screenshot ${rest[0]}`);
      });
    case 'quit':
      return withPage((page) => invoke(page, 'quit_app')).then(() => console.log('Quit.'));
    case 'kill': {
      const name = PROCESSES[rest[0] ?? 'app'];
      if (!name) throw new Error('kill app, engine or torrent');
      const r = spawnSync('taskkill', ['/F', '/IM', `${name}.exe`], { encoding: 'utf8' });
      console.log(`${clock()} ${r.stdout.trim() || r.stderr.trim()}`);
      return;
    }
    default:
      console.log('Commands: start, onboard, add, pause, resume, cancel, retry, remove, click, watch, status, log, export, shot, quit, kill. See tests/field/README.md.');
  }
}

main().catch((e) => {
  console.error(`field: ${e.message}`);
  process.exit(1);
});
