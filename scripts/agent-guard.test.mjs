import { spawnSync } from 'node:child_process';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';

// The hook as Claude Code runs it: a real process, JSON on stdin (#98).
const HOOK = fileURLToPath(new URL('./agent-guard.mjs', import.meta.url));
const run = (input) => spawnSync(process.execPath, [HOOK], { input: typeof input === 'string' ? input : JSON.stringify(input), encoding: 'utf8', timeout: 10000 });
const call = (command, cwd, tool = 'Bash') => run({ hook_event_name: 'PreToolUse', tool_name: tool, tool_input: { command }, cwd });

let repo;
beforeAll(() => {
  repo = mkdtempSync(join(tmpdir(), 'v2-guard-'));
  const git = (...args) => spawnSync('git', ['-C', repo, ...args], { encoding: 'utf8' });
  git('init', '-q', '-b', 'main');
  git('-c', 'user.name=t', '-c', 'user.email=t@example.invalid', 'commit', '-q', '--allow-empty', '-m', 'first');
});
afterAll(() => rmSync(repo, { recursive: true, force: true }));

describe('agent-guard hook', () => {
  it('prints a deny decision with its reason', () => {
    const r = call('git -C . push origin main', repo);
    expect(r.status).toBe(0);
    const out = JSON.parse(r.stdout);
    expect(out.hookSpecificOutput).toMatchObject({ hookEventName: 'PreToolUse', permissionDecision: 'deny' });
    expect(out.hookSpecificOutput.permissionDecisionReason).toMatch(/this pushes to main/);
  });

  it('prints nothing for a command it allows, so the normal permission flow decides', () => {
    const r = call('git push -u origin feat/41-range-probe', repo);
    expect(r.status).toBe(0);
    expect(r.stdout).toBe('');
  });

  it('reads the checked-out branch from the repository: a plain push on main is refused', () => {
    expect(JSON.parse(call('git push', repo).stdout).hookSpecificOutput.permissionDecision).toBe('deny');
  });

  it('checks PowerShell commands too', () => {
    expect(call('& git push origin main', repo, 'PowerShell').stdout).toMatch(/"deny"/);
  });

  it('ignores other tools', () => {
    const r = run({ hook_event_name: 'PreToolUse', tool_name: 'Read', tool_input: { file_path: 'x' }, cwd: repo });
    expect(r.status).toBe(0);
    expect(r.stdout).toBe('');
  });

  it('blocks the call when it fails, instead of failing open', () => {
    const r = run('not json');
    expect(r.status).toBe(2);
    expect(r.stderr).toMatch(/agent guard .* refuses it/);
  });
});
