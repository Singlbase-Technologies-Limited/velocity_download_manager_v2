import { describe, expect, it } from 'vitest';
import { checkCommand } from './agent-guard-lib.mjs';

// The current branch per directory, and the git aliases the user has configured.
const onFeature = { currentBranch: () => 'feat/41-range-probe', alias: () => null };
const onMain = { currentBranch: () => 'main', alias: () => null };
const bash = (command, ctx = onFeature) => checkCommand(command, { shell: 'bash', ...ctx });
const ps = (command, ctx = onFeature) => checkCommand(command, { shell: 'powershell', ...ctx });

describe('checkCommand allows ordinary work', () => {
  it.each([
    'git push -u origin feat/41-range-probe',
    'git push origin fix/12-main-menu',
    'git push origin main-menu',
    'git push origin feat/41:feat/41',
    'git push',
    'git push origin HEAD',
    'git status && git log --oneline -5',
    'git reset --soft HEAD~1',
    'git reset HEAD docs/README.md',
    'git fetch origin main && git merge origin/main',
    'git switch main && git pull --ff-only',
    'git log origin/main..HEAD',
    'echo "git push origin main"',
    "grep -n 'git reset --hard' AGENTS.md",
    'GH_TOKEN="$(gh auth token -u velocity-claude-agent)" gh pr create --base main --title x',
    "cat > notes.md <<'EOF'\ngit push origin main\ngit reset --hard\nEOF\nwc -l notes.md",
    'node scripts/check-pr.mjs --base main',
  ])('bash: %s', (command) => {
    expect(bash(command)).toEqual({ deny: false });
  });

  it.each([
    'git push -u origin feat/41-range-probe',
    'git status; git log -1',
    '$env:GH_TOKEN = (gh auth token -u velocity-claude-agent); gh pr list',
    'Write-Output "git push origin main"',
  ])('powershell: %s', (command) => {
    expect(ps(command)).toEqual({ deny: false });
  });
});

describe('checkCommand blocks pushes to main', () => {
  it.each([
    'git push origin main',
    'git push origin HEAD:main',
    'git push origin refs/heads/main',
    'git push origin feat/x:refs/heads/main',
    'git push origin :main',
    'git push origin --delete main',
    'git push -u origin main',
    'git push --all origin',
    'git push --mirror origin',
  ])('%s', (command) => {
    const r = bash(command);
    expect(r.deny).toBe(true);
    expect(r.reason).toMatch(/main/);
  });

  it.each(['git push', 'git push origin', 'git push origin HEAD', 'git push -u origin HEAD'])('%s while main is checked out', (command) => {
    expect(bash(command, onMain).deny).toBe(true);
  });

  // The forms Claude Code's permission docs say a text rule doesn't match.
  it.each([
    'git -C . push origin main',
    'git -c push.default=current push origin main',
    "git 'push' origin main",
    'git "push" origin "main"',
    'git --git-dir=.git --work-tree . push origin main',
    'git --no-pager push origin main',
    '/usr/bin/git push origin main',
    'git.exe push origin main',
    'FOO=1 git push origin main',
    'env git push origin main',
    'command git push origin main',
    'sudo git push origin main',
    "bash -c 'git push origin main'",
    'sh -c "git -C . push origin main"',
    "bash -lc 'cd x && git push origin main'",
    'cd repo && git push origin main',
    'git status; git push origin main',
    'false || git push origin main',
    'git log | git push origin main',
    'echo $(git push origin main)',
    'echo `git push origin main`',
    "eval 'git push origin main'",
    'cmd /c git push origin main',
  ])('bash bypass form: %s', (command) => {
    expect(bash(command).deny).toBe(true);
  });

  it.each([
    'git push origin main',
    '& git push origin main',
    '& "C:\\Program Files\\Git\\cmd\\git.exe" push origin main',
    'git status; git push origin main',
    'pwsh -c "git push origin main"',
    'powershell -NoProfile -Command "git -C . push origin main"',
    "git push origin `\n  main",
  ])('powershell: %s', (command) => {
    expect(ps(command).deny).toBe(true);
  });
});

describe('checkCommand blocks force pushes', () => {
  it.each([
    'git push --force origin feat/x',
    'git push -f origin feat/x',
    'git push origin feat/x --force',
    'git push --force-with-lease origin feat/x',
    'git push --force-with-lease=feat/x:abc123 origin feat/x',
    'git push --force-if-includes origin feat/x',
    'git push -uf origin feat/x',
    'git push origin +feat/x',
    'git push origin +HEAD:feat/x',
    'git -c x=y push --force origin some-branch',
  ])('%s', (command) => {
    const r = bash(command);
    expect(r.deny).toBe(true);
    expect(r.reason).toMatch(/force/i);
  });
});

describe('checkCommand blocks git reset --hard', () => {
  it.each(['git reset --hard', 'git reset --hard origin/main', 'git reset HEAD~1 --hard', 'git -C repo reset --hard', "bash -c 'git reset --hard'"])(
    '%s',
    (command) => {
      const r = bash(command);
      expect(r.deny).toBe(true);
      expect(r.reason).toMatch(/reset --hard/);
    },
  );
});

describe('checkCommand resolves git aliases', () => {
  it('blocks an alias defined on the command line', () => {
    expect(bash('git -c alias.p=push p origin main').deny).toBe(true);
    expect(bash("git -c 'alias.nuke=reset --hard' nuke").deny).toBe(true);
  });
  it('blocks a configured alias', () => {
    const aliases = { p: 'push --force', up: '!git push origin main' };
    const ctx = { ...onFeature, alias: (name) => aliases[name] ?? null };
    expect(bash('git p origin feat/x', ctx).deny).toBe(true);
    expect(bash('git up', ctx).deny).toBe(true);
  });
  it('allows a configured alias that does nothing guarded', () => {
    const ctx = { ...onFeature, alias: (name) => (name === 'st' ? 'status -sb' : null) };
    expect(bash('git st', ctx)).toEqual({ deny: false });
  });
});

describe('checkCommand denies what it cannot read', () => {
  it.each(['git push origin "main', "git reset 'oops"])('an unbalanced quote that mentions git push or reset: %s', (command) => {
    const r = bash(command);
    expect(r.deny).toBe(true);
    expect(r.reason).toMatch(/couldn't read/);
  });
  it('allows an unreadable command that mentions neither', () => {
    expect(bash('echo "unbalanced')).toEqual({ deny: false });
  });
});

describe('checkCommand explains each refusal', () => {
  it('names the rule and the safe alternative', () => {
    const r = bash('git -C . push origin main');
    expect(r.reason).toMatch(/agent-guard/);
    expect(r.reason).toMatch(/pull request/);
    expect(r.reason).toContain('git -C . push origin main');
  });
});
