#!/usr/bin/env node
// The agent guard (#98): a Claude Code PreToolUse hook for the Bash and PowerShell tools,
// registered in .claude/settings.json. It refuses a command that pushes to `main`, force-pushes or
// runs `git reset --hard`, in any spelling, and a `gh pr create` or `gh pr edit` whose description doesn't follow
// .github/PULL_REQUEST_TEMPLATE.md (PO decision on #59). The rules are in agent-guard-lib.mjs, unit tested.
// The deny rules in .claude/settings.json stay as the first layer.
//
// Hook protocol (https://code.claude.com/docs/en/hooks#pretooluse-decision-control): the tool call
// arrives as JSON on stdin. A refusal is printed as a `permissionDecision: "deny"` with its reason;
// no output lets the normal permission flow decide. Claude Code treats a crash as "allow"
// (fail-open), so any internal error here exits 2, which blocks the call: when in doubt, deny.
import { spawnSync } from 'node:child_process';
import { existsSync, readFileSync, statSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { checkCommand } from './agent-guard-lib.mjs';

/** Git Bash writes Windows paths as /c/projects/…; Node needs C:/projects/…. */
const nativePath = (p) => (process.platform === 'win32' ? p.replace(/^\/([A-Za-z])(\/|$)/, '$1:/') : p);

const directory = (cwd, dirs) => dirs.reduce((base, d) => resolve(base, nativePath(d)), cwd);

function git(cwd, dirs, args) {
  const r = spawnSync('git', ['-C', directory(cwd, dirs), ...args], { encoding: 'utf8', windowsHide: true, timeout: 5000 });
  return r.status === 0 ? r.stdout.trim() : null;
}

/**
 * The checked-out branch, read from HEAD in the repository's git directory: about 50 ms faster
 * than starting git on Windows, which keeps the hook under its 100 ms budget. Returns undefined
 * when the layout isn't the plain one (a `.git` folder or a `gitdir:` file), so git is asked.
 */
function branchFromHead(start) {
  for (let dir = start; ; dir = dirname(dir)) {
    const dotGit = join(dir, '.git');
    if (existsSync(dotGit)) {
      let gitDir = dotGit;
      if (statSync(dotGit).isFile()) {
        const m = /^gitdir: (.+)$/m.exec(readFileSync(dotGit, 'utf8'));
        if (!m) return undefined;
        gitDir = resolve(dir, m[1].trim());
      }
      const head = readFileSync(join(gitDir, 'HEAD'), 'utf8').trim();
      const ref = /^ref: refs\/heads\/(.+)$/.exec(head);
      return ref ? ref[1] : null; // a detached HEAD has no branch
    }
    if (dirname(dir) === dir) return undefined;
  }
}

function currentBranch(cwd, dirs) {
  const fromHead = branchFromHead(directory(cwd, dirs));
  return fromHead !== undefined ? fromHead : git(cwd, dirs, ['symbolic-ref', '--quiet', '--short', 'HEAD']) || null;
}

/** A text file, from the command's folder; null when it can't be read. */
function readText(cwd, dirs, path) {
  try {
    return readFileSync(resolve(directory(cwd, dirs), nativePath(path)), 'utf8');
  } catch {
    return null;
  }
}

/** The PR template of the repository the command runs in, from the nearest folder that has one; null when none does. */
function prTemplate(cwd, dirs) {
  for (let dir = directory(cwd, dirs); ; dir = dirname(dir)) {
    const path = join(dir, '.github', 'PULL_REQUEST_TEMPLATE.md');
    if (existsSync(path)) return readText(dir, [], path);
    if (dirname(dir) === dir) return null;
  }
}

function readStdin() {
  return new Promise((done, fail) => {
    let data = '';
    process.stdin.setEncoding('utf8');
    process.stdin.on('data', (chunk) => (data += chunk));
    process.stdin.on('end', () => done(data));
    process.stdin.on('error', fail);
  });
}

async function main() {
  const input = JSON.parse(await readStdin());
  const tool = input.tool_name;
  if (tool !== 'Bash' && tool !== 'PowerShell') return;
  const command = input.tool_input?.command;
  if (typeof command !== 'string' || !command.trim()) return;
  const cwd = nativePath(input.cwd || process.cwd());
  const result = checkCommand(command, {
    shell: tool === 'PowerShell' ? 'powershell' : 'bash',
    // A detached HEAD or a folder outside a repository gives null, which the guard treats as unsure.
    currentBranch: (dirs) => currentBranch(cwd, dirs),
    alias: (name, dirs) => git(cwd, dirs, ['config', '--get', `alias.${name}`]) || null,
    readFile: (path, dirs) => readText(cwd, dirs, path),
    template: (dirs) => prTemplate(cwd, dirs),
  });
  if (result.deny) {
    process.stdout.write(
      `${JSON.stringify({ hookSpecificOutput: { hookEventName: 'PreToolUse', permissionDecision: 'deny', permissionDecisionReason: result.reason } })}\n`,
    );
  }
}

main().catch((e) => {
  process.stderr.write(`Blocked by the agent guard (scripts/agent-guard.mjs, #98): it failed while checking this command (${e instanceof Error ? e.message : e}), so it refuses it rather than let it through.\n`);
  process.exit(2);
});
