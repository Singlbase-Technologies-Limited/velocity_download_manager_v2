#!/usr/bin/env node
// Downloads the native sources a target needs and proves each one against the lock file (#35,
// docs/BUILDING_NATIVE.md): the archive's SHA-256, then its detached signature, checked by gpg
// against only that library's pinned key. Nothing is extracted or built from a file that failed.
//
//   node scripts/native/fetch.mjs [--target <rust-target-triple>]
//
// The rules live in native-lib.mjs (unit tested); this file does the downloads and runs gpg.
import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, renameSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { checkDownload, checkSignature, librariesFor, parseLock } from './native-lib.mjs';

const HERE = dirname(fileURLToPath(import.meta.url));
export const REPO = join(HERE, '..', '..');
export const NATIVE_DIR = join(REPO, 'target', 'native');
const DOWNLOAD_TIMEOUT_MS = 10 * 60 * 1000;

export function readLock() {
  return parseLock(readFileSync(join(HERE, 'deps.lock'), 'utf8'));
}

/** The Rust host triple, which is the target the native build serves. */
export function hostTarget() {
  const r = spawnSync('rustc', ['-vV'], { encoding: 'utf8' });
  const m = /^host: (\S+)$/m.exec(r.stdout ?? '');
  if (r.status !== 0 || !m) throw new Error(`could not read the host target from \`rustc -vV\`: ${r.stderr || r.error?.message}`);
  return m[1];
}

/**
 * gpg on PATH, or the copy Git for Windows ships (it isn't on the Windows PATH by default), and how
 * it takes paths: Git's gpg is an MSYS program that reads only POSIX paths (`/c/x`), which its
 * `gpg --version` shows with a `Home:` starting with `/`.
 */
function gpgCommand() {
  const candidates = ['gpg'];
  if (process.platform === 'win32') candidates.push('C:\\Program Files\\Git\\usr\\bin\\gpg.exe');
  for (const cmd of candidates) {
    const r = spawnSync(cmd, ['--version'], { encoding: 'utf8' });
    if (r.status !== 0) continue;
    const msys = process.platform === 'win32' && /^Home: \//m.test(r.stdout);
    const path = msys ? (p) => p.replace(/^([A-Za-z]):[\\/]/, (_, d) => `/${d.toLowerCase()}/`).replaceAll('\\', '/') : (p) => p;
    return { cmd, path };
  }
  throw new Error('gpg was not found. Install GnuPG (Linux: `sudo apt install gnupg`; Windows: it comes with Git for Windows)');
}

const sha256File = (path) => createHash('sha256').update(readFileSync(path)).digest('hex');
const baseName = (url) => url.slice(url.lastIndexOf('/') + 1);

async function download(url, dest) {
  const res = await fetch(url, { redirect: 'follow', signal: AbortSignal.timeout(DOWNLOAD_TIMEOUT_MS) });
  if (!res.ok) throw new Error(`download of ${url} failed: HTTP ${res.status}`);
  // `.part` first, so an interrupted download is never mistaken for a finished one.
  const part = `${dest}.part`;
  writeFileSync(part, Buffer.from(await res.arrayBuffer()));
  renameSync(part, dest);
}

/** Runs gpg in a fresh, empty home holding only `keyFile`, and returns its status output. */
function verifyWithKey(...paths) {
  const { cmd: gpg, path: arg } = gpgCommand();
  const homeDir = mkdtempSync(join(tmpdir(), 'v2-gpg-'));
  const home = arg(homeDir);
  const [keyFile, sigFile, dataFile] = paths.map(arg);
  try {
    const imported = spawnSync(gpg, ['--batch', '--no-autostart', '--homedir', home, '--import', keyFile], { encoding: 'utf8' });
    if (imported.status !== 0) throw new Error(`gpg could not import ${keyFile}: ${imported.stderr}`);
    const verified = spawnSync(gpg, ['--batch', '--no-autostart', '--homedir', home, '--status-fd', '1', '--verify', sigFile, dataFile], { encoding: 'utf8' });
    return verified.stdout ?? '';
  } finally {
    rmSync(homeDir, { recursive: true, force: true });
  }
}

/**
 * Makes every source `target` needs present in target/native/sources and proven.
 * @returns {Promise<{lock: ReturnType<typeof parseLock>, files: Record<string, string>, signatures: Record<string, string>}>}
 */
export async function fetchSources(target, log = console.log) {
  const lock = readLock();
  const dir = join(NATIVE_DIR, 'sources');
  mkdirSync(dir, { recursive: true });
  const files = {};
  const signatures = {};
  for (const name of librariesFor(target).libraries) {
    const entry = lock.find((e) => e.name === name);
    if (!entry) throw new Error(`${name}: ${target} needs it, but scripts/native/deps.lock doesn't pin it`);
    const file = join(dir, baseName(entry.url));
    if (!existsSync(file) || sha256File(file) !== entry.sha256) {
      log(`${name}: downloading ${entry.url}`);
      rmSync(file, { force: true });
      await download(entry.url, file);
    }
    try {
      checkDownload(entry, sha256File(file));
    } catch (e) {
      rmSync(file, { force: true });
      throw e;
    }
    let status = null;
    if (entry.signature.status === 'verified') {
      const sig = `${file}.asc`;
      if (!existsSync(sig)) await download(`${entry.url}.asc`, sig);
      status = verifyWithKey(join(HERE, 'keys', `${name}.asc`), sig, file);
    }
    signatures[name] = checkSignature(entry, status);
    log(`${name} ${entry.version}: SHA-256 matches, signature ${signatures[name]}`);
    files[name] = file;
  }
  return { lock, files, signatures };
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  const argv = process.argv.slice(2).filter((a) => a !== '--');
  const target = argv[0] === '--target' ? argv[1] : hostTarget();
  fetchSources(target).catch((e) => {
    console.error(`native fetch failed: ${e instanceof Error ? e.message : e}`);
    process.exit(1);
  });
}
