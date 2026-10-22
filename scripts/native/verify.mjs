#!/usr/bin/env node
// Release blocker (#35, docs/DEPENDENCIES.md "Release blockers" items 3 to 5): fails unless the
// native build for the Rust host target has a complete build-info.json that matches the lock file,
// and every artefact it records still has its recorded SHA-256. `pnpm tauri build` runs this first
// (src-tauri/tauri.conf.json, beforeBuildCommand), so no package is built without it.
//
//   node scripts/native/verify.mjs
import { createHash } from 'node:crypto';
import { existsSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { checkArtifacts, checkBuildInfo } from './native-lib.mjs';
import { NATIVE_DIR, hostTarget, readLock } from './fetch.mjs';

function main() {
  const target = hostTarget();
  const outDir = join(NATIVE_DIR, target);
  const infoPath = join(outDir, 'build-info.json');
  if (!existsSync(infoPath)) {
    return [`${infoPath} is missing. Run \`pnpm native:build\` (docs/BUILDING_NATIVE.md)`];
  }
  let info;
  try {
    info = JSON.parse(readFileSync(infoPath, 'utf8'));
  } catch (e) {
    return [`${infoPath} is not valid JSON (${e instanceof Error ? e.message : e}). Run \`pnpm native:build --force\``];
  }
  const hashOf = (rel) => {
    const file = join(outDir, 'prefix', ...rel.split('/'));
    return existsSync(file) ? createHash('sha256').update(readFileSync(file)).digest('hex') : null;
  };
  const problems = [...checkBuildInfo(info, readLock()), ...checkArtifacts(info, hashOf)];
  if (!problems.length) console.log(`native build for ${target} verified: ${infoPath} matches scripts/native/deps.lock and its artefacts`);
  return problems;
}

try {
  const problems = main();
  if (problems.length) {
    console.error(`native:verify failed, so no release can be built (docs/DEPENDENCIES.md, "Release blockers"):\n- ${problems.join('\n- ')}`);
    process.exit(1);
  }
} catch (e) {
  console.error(`native:verify failed: ${e instanceof Error ? e.message : e}`);
  process.exit(1);
}
