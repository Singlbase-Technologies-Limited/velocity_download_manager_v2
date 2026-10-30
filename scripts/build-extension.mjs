#!/usr/bin/env node
// `pnpm build:extension [-- --release]`: builds the browser extension (#64) for Chromium and
// Firefox into browser-extension/dist/<target>, with Vite. A development build talks to the
// development app's native host; `--release` to the release app's. See docs/BROWSER_EXTENSION.md.
import { copyFileSync, cpSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { build } from 'vite';
import { ENTRIES, ICONS, MINIMUM_CHROME_VERSION, MINIMUM_FIREFOX_VERSION, TARGETS, manifest } from './extension-lib.mjs';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const extension = join(root, 'browser-extension');
const release = process.argv.includes('--release');
const branding = JSON.parse(readFileSync(join(root, 'branding.json'), 'utf8'));
const { version } = JSON.parse(readFileSync(join(root, 'package.json'), 'utf8'));

for (const target of TARGETS) {
  const out = join(extension, 'dist', target);
  rmSync(out, { recursive: true, force: true });
  mkdirSync(join(out, 'icons'), { recursive: true });
  // One build per script: each is a classic script (an IIFE) with nothing shared at run time,
  // since Firefox's background scripts and content scripts can't be modules.
  for (const entry of ENTRIES) {
    await build({
      configFile: false,
      publicDir: false,
      logLevel: 'warn',
      define: { __VELOCITY_RELEASE__: JSON.stringify(release) },
      build: {
        outDir: out,
        emptyOutDir: false,
        minify: false,
        sourcemap: false,
        target: target === 'chrome' ? `chrome${MINIMUM_CHROME_VERSION}` : `firefox${MINIMUM_FIREFOX_VERSION.split('.')[0]}`,
        lib: { entry: join(extension, 'src', `${entry}.ts`), formats: ['iife'], name: `velocity_${entry}`, fileName: () => `${entry}.js` },
      },
    });
  }
  cpSync(join(extension, 'static'), out, { recursive: true });
  for (const [size, source] of Object.entries(ICONS)) {
    copyFileSync(join(root, 'src-tauri', 'icons', source), join(out, 'icons', `icon-${size}.png`));
  }
  writeFileSync(join(out, 'manifest.json'), `${JSON.stringify(manifest(target, { release, version, branding }), null, 2)}\n`);
  console.log(`built ${target}${release ? ' (release)' : ''} -> ${out}`);
}
