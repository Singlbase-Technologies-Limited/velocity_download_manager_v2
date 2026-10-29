// Pure logic for the agent guard (#98): decides whether a Bash or PowerShell command an agent is
// about to run pushes to `main`, force-pushes or runs `git reset --hard`, in any spelling. The
// deny rules in .claude/settings.json match command text only, so forms such as
// `git -C . push origin main` or `bash -c '…'` get past them; this reads the command the way a
// shell would. scripts/agent-guard.mjs is the PreToolUse hook that calls it.
//
// It also holds `gh pr create` and `gh pr edit` to .github/PULL_REQUEST_TEMPLATE.md (PO decision on #59): the
// description must come from a file, through --body-file, and pass scripts/pr-body-lib.mjs, as must the title.
//
// When in doubt, deny: a command it can't read that mentions git with push or reset, or gh pr create or edit, is refused.

import { checkPrBody, checkPrTitle } from './pr-body-lib.mjs';

const MAX_DEPTH = 8;
const PROTECTED_BRANCH = 'main';

/** git subcommands that aliases can't shadow, so they need no alias lookup. */
const GIT_BUILTINS = new Set(
  (
    'add am apply archive bisect blame branch bundle cat-file check-ignore checkout cherry cherry-pick clean clone commit config ' +
    'count-objects credential describe diff difftool fetch for-each-ref format-patch fsck gc grep hash-object help init log ' +
    'ls-files ls-remote ls-tree maintenance merge merge-base mv name-rev notes pull push range-diff rebase reflog remote reset ' +
    'restore rev-list rev-parse revert rm shortlog show show-ref sparse-checkout stash status submodule switch symbolic-ref tag ' +
    'update-index update-ref var verify-commit version worktree'
  ).split(' '),
);
/** git global options that take the next word as their value. */
const GIT_VALUE_OPTIONS = new Set(['-C', '-c', '--git-dir', '--work-tree', '--namespace', '--super-prefix', '--config-env', '--list-cmds']);
/** `git push` options that take the next word as their value. */
const PUSH_VALUE_OPTIONS = new Set(['-o', '--push-option', '--repo', '--receive-pack', '--exec']);
/** Programs that only run the command after them (their own options are skipped). */
const WRAPPERS = new Set(['env', 'sudo', 'command', 'exec', 'nohup', 'time', 'nice', 'builtin', 'doas']);
const SHELLS = new Set(['bash', 'sh', 'zsh', 'dash', 'ksh']);
const POWERSHELLS = new Set(['pwsh', 'powershell']);

class Unreadable extends Error {}

// ---------- Lexing ----------

/**
 * Splits shell text into simple commands (arrays of words) and the texts of command
 * substitutions, which run too. Throws Unreadable on an unbalanced quote or substitution.
 * @param {'bash' | 'powershell'} shell
 * @returns {{commands: string[][], nested: string[]}}
 */
function lex(text, shell) {
  const ps = shell === 'powershell';
  const commands = [];
  const nested = [];
  let words = [];
  let word = '';
  let inWord = false;
  const heredocs = [];
  const endWord = () => {
    if (inWord) words.push(word);
    word = '';
    inWord = false;
  };
  const endCommand = () => {
    endWord();
    if (words.length) commands.push(words);
    words = [];
  };
  // Reads to the `)` matching an opened `$(`, honouring nesting and quotes; returns its index.
  const closeParen = (from) => {
    let depth = 1;
    for (let j = from; j < text.length; j++) {
      const ch = text[j];
      if (ch === "'" || ch === '"') {
        const end = text.indexOf(ch, j + 1);
        if (end < 0) throw new Unreadable('unbalanced quote');
        j = end;
      } else if (ch === '(') depth++;
      else if (ch === ')' && --depth === 0) return j;
    }
    throw new Unreadable('unbalanced $(');
  };

  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    const next = text[i + 1];

    // Escapes: a backslash in bash, a backtick in PowerShell. An escaped newline continues the line.
    if ((!ps && c === '\\') || (ps && c === '`')) {
      if (next === '\n') i++;
      else if (next === '\r' && text[i + 2] === '\n') i += 2;
      else if (next !== undefined) {
        word += next;
        inWord = true;
        i++;
      }
      continue;
    }
    if (c === "'") {
      // Single quotes are literal; PowerShell writes a quote inside them as ''.
      let j = i + 1;
      for (;;) {
        const end = text.indexOf("'", j);
        if (end < 0) throw new Unreadable('unbalanced quote');
        word += text.slice(j, end);
        if (ps && text[end + 1] === "'") {
          word += "'";
          j = end + 2;
          continue;
        }
        i = end;
        break;
      }
      inWord = true;
      continue;
    }
    if (c === '"') {
      let j = i + 1;
      for (;;) {
        if (j >= text.length) throw new Unreadable('unbalanced quote');
        const d = text[j];
        if (d === '"' && ps && text[j + 1] === '"') {
          word += '"';
          j += 2;
        } else if (d === '"') break;
        else if (!ps && d === '\\' && '"\\$`\n'.includes(text[j + 1] ?? '')) {
          if (text[j + 1] !== '\n') word += text[j + 1];
          j += 2;
        } else if (ps && d === '`' && j + 1 < text.length) {
          word += text[j + 1];
          j += 2;
        } else if (d === '$' && text[j + 1] === '(') {
          const end = closeParen(j + 2);
          nested.push(text.slice(j + 2, end));
          j = end + 1;
        } else if (!ps && d === '`') {
          const end = text.indexOf('`', j + 1);
          if (end < 0) throw new Unreadable('unbalanced backtick');
          nested.push(text.slice(j + 1, end));
          j = end + 1;
        } else {
          word += d;
          j++;
        }
      }
      i = j;
      inWord = true;
      continue;
    }
    if (!ps && c === '`') {
      const end = text.indexOf('`', i + 1);
      if (end < 0) throw new Unreadable('unbalanced backtick');
      nested.push(text.slice(i + 1, end));
      inWord = true;
      i = end;
      continue;
    }
    // PowerShell here-strings, @'…'@ and @"…"@, are one word.
    if (ps && c === '@' && (next === "'" || next === '"') && !inWord) {
      const close = `\n${next}@`;
      const end = text.indexOf(close, i + 2);
      if (end < 0) throw new Unreadable('unbalanced here-string');
      word += text.slice(i + 2, end);
      inWord = true;
      i = end + close.length - 1;
      continue;
    }
    // Bash heredocs: the body is data, not commands; it starts after this line ends.
    if (!ps && c === '<' && next === '<' && text[i + 2] !== '<') {
      endWord();
      let j = i + 2;
      const strip = text[j] === '-';
      if (strip) j++;
      while (text[j] === ' ' || text[j] === '\t') j++;
      let delim = '';
      while (j < text.length && !/[\s;&|<>()]/.test(text[j])) {
        if (text[j] === "'" || text[j] === '"') {
          const end = text.indexOf(text[j], j + 1);
          if (end < 0) throw new Unreadable('unbalanced quote');
          delim += text.slice(j + 1, end);
          j = end + 1;
        } else delim += text[j++];
      }
      if (!delim) throw new Unreadable('heredoc without a delimiter');
      heredocs.push({ delim, strip });
      i = j - 1;
      continue;
    }
    if (c === '\n' || c === '\r') {
      endCommand();
      if (c === '\r' && next === '\n') i++;
      while (heredocs.length) {
        const { delim, strip } = heredocs.shift();
        let found = false;
        while (i + 1 < text.length) {
          const lineEnd = text.indexOf('\n', i + 1);
          const line = text.slice(i + 1, lineEnd < 0 ? text.length : lineEnd).replace(/\r$/, '');
          i = lineEnd < 0 ? text.length : lineEnd;
          if ((strip ? line.replace(/^\t+/, '') : line) === delim) {
            found = true;
            break;
          }
        }
        if (!found) throw new Unreadable('unterminated heredoc');
      }
      continue;
    }
    if (c === ' ' || c === '\t') {
      endWord();
      continue;
    }
    if (c === ';' || c === '|' || c === '(' || c === ')') {
      endCommand();
      if (c === '|' && next === '|') i++;
      continue;
    }
    if (c === '&') {
      // `2>&1` and `&>` are redirections; PowerShell's `&` at the start of a command calls it.
      if (word.endsWith('>') || word.endsWith('<') || next === '>') {
        word += c;
        inWord = true;
        continue;
      }
      if (next === '&') {
        endCommand();
        i++;
        continue;
      }
      if (ps) {
        endWord();
        continue;
      }
      endCommand();
      continue;
    }
    word += c;
    inWord = true;
  }
  if (heredocs.length) throw new Unreadable('unterminated heredoc');
  endCommand();
  return { commands: commands.map(dropRedirections), nested };
}

/** Removes redirections (`2>&1`, `> file`, `<in`) so they aren't read as arguments. */
function dropRedirections(words) {
  const out = [];
  for (let i = 0; i < words.length; i++) {
    const w = words[i];
    if (/^(\d*|&)(>>?|<)(&\d*)?$/.test(w)) {
      i++; // the target is the next word
      continue;
    }
    if (/^(\d*|&)(>>?|<)/.test(w)) continue;
    out.push(w);
  }
  return out;
}

// ---------- Decisions ----------

const basename = (p) => p.split(/[\\/]/).pop().toLowerCase().replace(/\.(exe|cmd|bat)$/, '');

function deny(why, advice) {
  return { why, advice };
}

const ADVICE = {
  main: 'Push your own branch and open a pull request into main instead (AGENTS.md, "Workflow")',
  force: 'Never force-push: push a new commit instead, or ask the PO',
  reset: '`git reset --hard` discards work: use `git restore <file>` or `git stash`, or ask the PO',
  unsure: 'Rewrite the command so it can be read (balanced quotes, no unusual wrappers), or ask the PO',
  pr: 'Write the description from .github/PULL_REQUEST_TEMPLATE.md to a file, check it with `pnpm check:pr-body <file> --title "…"`, and pass it with --body-file and a literal path (PO decision on #59)',
};

/** Checks one shell text; returns the first refusal, or null. */
function checkText(text, shell, ctx, depth) {
  if (depth > MAX_DEPTH) return deny('the command nests shells too deeply to check', ADVICE.unsure);
  let lexed;
  try {
    lexed = lex(text, shell);
  } catch (e) {
    if (!(e instanceof Unreadable)) throw e;
    const mentions = (/\bgit\b/i.test(text) && /\b(push|reset)\b/i.test(text)) || /\bgh\b[\s\S]*\bpr\s+(create|edit)\b/i.test(text);
    return mentions ? deny(`the guard couldn't read this command (${e.message}), and it mentions git push or reset, or gh pr create or edit`, ADVICE.unsure) : null;
  }
  for (const inner of lexed.nested) {
    const r = checkText(inner, shell, ctx, depth + 1);
    if (r) return r;
  }
  let dir = [];
  for (const words of lexed.commands) {
    const r = checkWords(words, shell, { ...ctx, dir }, depth);
    if (r) return r;
    // `cd x && git push` pushes from x.
    const exe = words[0] ? basename(words[0]) : '';
    if ((exe === 'cd' || exe === 'set-location' || exe === 'pushd' || exe === 'sl') && words[1]) dir = [...dir, words[1]];
  }
  return null;
}

function checkWords(input, shell, ctx, depth) {
  let words = [...input];
  // Leading assignments (FOO=1 git …) and wrappers (env, sudo, PowerShell's & and .).
  for (;;) {
    if (!words.length) return null;
    const w = words[0];
    if (/^[A-Za-z_][A-Za-z0-9_]*=/.test(w) || w === '&' || w === '.') {
      words = words.slice(1);
    } else if (WRAPPERS.has(basename(w))) {
      words = words.slice(1);
      while (words.length && (words[0].startsWith('-') || /^[A-Za-z_][A-Za-z0-9_]*=/.test(words[0]))) words = words.slice(1);
    } else break;
  }
  const exe = basename(words[0]);
  const rest = words.slice(1);

  if (SHELLS.has(exe)) {
    const flag = rest.findIndex((a) => /^-[A-Za-z]*c[A-Za-z]*$/.test(a));
    return flag >= 0 && rest[flag + 1] !== undefined ? checkText(rest[flag + 1], 'bash', ctx, depth + 1) : null;
  }
  if (POWERSHELLS.has(exe)) {
    const enc = rest.findIndex((a) => /^-e(nc(odedcommand)?)?$/i.test(a));
    if (enc >= 0 && rest[enc + 1]) {
      const decoded = Buffer.from(rest[enc + 1], 'base64').toString('utf16le');
      return checkText(decoded, 'powershell', ctx, depth + 1);
    }
    const flag = rest.findIndex((a) => /^-c(o(m(m(a(n(d)?)?)?)?)?)?$/i.test(a));
    return flag >= 0 ? checkText(rest.slice(flag + 1).join(' '), 'powershell', ctx, depth + 1) : null;
  }
  if (exe === 'cmd') {
    const flag = rest.findIndex((a) => /^\/[ck]$/i.test(a));
    return flag >= 0 ? checkText(rest.slice(flag + 1).join(' '), 'bash', ctx, depth + 1) : null;
  }
  if (exe === 'eval' || exe === 'invoke-expression' || exe === 'iex') return checkText(rest.join(' '), shell, ctx, depth + 1);
  if (exe === 'git') return checkGit(rest, ctx, depth);
  if (exe === 'gh') return checkGh(rest, ctx);
  return null;
}

/** `gh pr create` and `gh pr edit` options that write the description some other way than a file the guard can read. */
const PR_BODY_ELSEWHERE = new Set(['--body', '-b', '--fill', '-f', '--fill-first', '--fill-verbose', '--web', '-w', '--editor', '-e', '--template', '-T']);

/** The value of option `long` or `short` in `args`, as `--x v`, `-x v` or `--x=v`; undefined when absent. */
function optionValue(args, long, short) {
  for (let i = 0; i < args.length; i++) {
    if (args[i] === long || args[i] === short) return args[i + 1] ?? '';
    if (args[i].startsWith(`${long}=`)) return args[i].slice(long.length + 1);
  }
  return undefined;
}

/** `gh pr create` and `gh pr edit` follow the PR template (PO decision on #59); any other gh command passes. */
function checkGh(args, ctx) {
  const pr = args.indexOf('pr');
  const sub = pr >= 0 ? args[pr + 1] : undefined;
  if (sub !== 'create' && sub !== 'edit') return null;
  const opts = args.slice(pr + 2);
  const elsewhere = opts.find((a) => PR_BODY_ELSEWHERE.has(a.split('=')[0]));
  if (elsewhere) return deny(`\`gh pr ${sub}\` with ${elsewhere} writes a description the guard can't check; use --body-file`, ADVICE.pr);
  const file = optionValue(opts, '--body-file', '-F');
  const title = optionValue(opts, '--title', '-t');
  if (file === undefined) {
    if (sub === 'create') return deny('`gh pr create` needs the description from --body-file', ADVICE.pr);
    const problems = title === undefined ? [] : checkPrTitle(title, '');
    return problems.length ? deny(`the PR title doesn't follow the rules: ${problems.join(' ')}`, ADVICE.pr) : null;
  }
  if (!file || file === '-') return deny('the description must come from a file the guard can read, not standard input', ADVICE.pr);
  const body = ctx.readFile?.(file, ctx.dir ?? []) ?? null;
  const template = ctx.template?.(ctx.dir ?? []) ?? null;
  if (body === null) return deny(`the guard couldn't read ${file}, so it can't check the description`, ADVICE.pr);
  if (template === null) return deny("the guard couldn't find .github/PULL_REQUEST_TEMPLATE.md to check the description against", ADVICE.pr);
  if (sub === 'create' && title === undefined) return deny('`gh pr create` needs --title, checked with the description', ADVICE.pr);
  const problems = [...checkPrBody(body, template), ...(title === undefined ? [] : checkPrTitle(title, body))];
  return problems.length ? deny(`the PR description doesn't follow .github/PULL_REQUEST_TEMPLATE.md: ${problems.join(' ')}`, ADVICE.pr) : null;
}

function checkGit(args, ctx, depth) {
  const config = {};
  let dir = ctx.dir ?? [];
  let i = 0;
  for (; i < args.length; i++) {
    const a = args[i];
    if (!a.startsWith('-')) break;
    const eq = a.indexOf('=');
    const name = eq > 0 ? a.slice(0, eq) : a;
    const value = eq > 0 ? a.slice(eq + 1) : GIT_VALUE_OPTIONS.has(a) ? args[++i] : undefined;
    if (name === '-C' && value !== undefined) dir = [...dir, value];
    if ((name === '-c' || name === '--config-env') && value !== undefined) {
      const sep = value.indexOf('=');
      if (sep > 0) config[value.slice(0, sep).toLowerCase()] = value.slice(sep + 1);
    }
  }
  const sub = args[i];
  const rest = args.slice(i + 1);
  if (sub === undefined) return null;
  const sctx = { ...ctx, dir };

  // A refspec configured on the command line is pushed by a plain `git push`.
  for (const [key, value] of Object.entries(config)) {
    if (/^remote\..+\.push$/.test(key) && sub === 'push') {
      const r = checkRefspecs([value], sctx, false);
      if (r) return r;
    }
  }

  if (!GIT_BUILTINS.has(sub)) {
    const expansion = config[`alias.${sub.toLowerCase()}`] ?? ctx.alias(sub, dir);
    if (expansion) {
      if (depth > MAX_DEPTH) return deny('the git aliases nest too deeply to check', ADVICE.unsure);
      if (expansion.startsWith('!')) return checkText(`${expansion.slice(1)} ${rest.join(' ')}`, 'bash', sctx, depth + 1);
      let words;
      try {
        words = lex(expansion, 'bash').commands[0] ?? [];
      } catch {
        return deny(`the guard couldn't read the git alias "${sub}"`, ADVICE.unsure);
      }
      return checkGit([...words, ...rest], sctx, depth + 1);
    }
    return null;
  }
  if (sub === 'reset') return rest.includes('--hard') ? deny('`git reset --hard` discards uncommitted work', ADVICE.reset) : null;
  if (sub === 'push') return checkPush(rest, sctx);
  return null;
}

function checkPush(args, ctx) {
  const positional = [];
  let force = false;
  let all = false;
  let tags = false;
  let del = false;
  for (let i = 0; i < args.length; i++) {
    const a = args[i];
    if (a === '--') {
      positional.push(...args.slice(i + 1));
      break;
    }
    if (!a.startsWith('-') || a === '-') {
      positional.push(a);
      continue;
    }
    const name = a.includes('=') ? a.slice(0, a.indexOf('=')) : a;
    if (PUSH_VALUE_OPTIONS.has(a)) i++;
    else if (['--force', '--force-with-lease', '--force-if-includes'].includes(name)) force = true;
    else if (name === '--all' || name === '--mirror' || name === '--branches') all = true;
    else if (name === '--tags') tags = true;
    else if (name === '--delete') del = true;
    else if (/^-[A-Za-z]+$/.test(a)) {
      if (a.includes('f')) force = true;
      if (a.includes('d')) del = true;
    }
  }
  if (force) return deny('force pushes rewrite history on the remote', ADVICE.force);
  if (all) return deny(`--all, --branches and --mirror push ${PROTECTED_BRANCH} too`, ADVICE.main);
  const [, ...refspecs] = positional;
  if (!refspecs.length) {
    if (tags) return null;
    return pushesCurrent(ctx);
  }
  return checkRefspecs(refspecs, ctx, del);
}

/** Whether pushing the current branch would update main (a plain `git push` or `HEAD`). */
function pushesCurrent(ctx) {
  const branch = ctx.currentBranch(ctx.dir ?? []);
  if (branch === null || branch === undefined) return deny("the guard couldn't tell which branch is checked out, so it can't rule out main", ADVICE.unsure);
  return branch === PROTECTED_BRANCH ? deny(`the current branch is ${PROTECTED_BRANCH}, so this pushes to ${PROTECTED_BRANCH}`, ADVICE.main) : null;
}

function checkRefspecs(refspecs, ctx, del) {
  for (const raw of refspecs) {
    if (raw.startsWith('+')) return deny(`the refspec ${raw} forces the update`, ADVICE.force);
    let dst;
    if (del) dst = raw;
    else if (raw.includes(':')) dst = raw.slice(raw.indexOf(':') + 1);
    else dst = raw;
    if (!del && !raw.includes(':') && (raw === 'HEAD' || raw === '@')) {
      const r = pushesCurrent(ctx);
      if (r) return r;
      continue;
    }
    if (dst.replace(/^refs\/heads\//, '') === PROTECTED_BRANCH) {
      return deny(del ? `this deletes ${PROTECTED_BRANCH} on the remote` : `this pushes to ${PROTECTED_BRANCH}`, ADVICE.main);
    }
  }
  return null;
}

/**
 * @param {string} command the command the agent is about to run
 * @param {{shell: 'bash' | 'powershell', currentBranch: (dir: string[]) => string | null,
 *   alias: (name: string, dir: string[]) => string | null,
 *   readFile?: (path: string, dir: string[]) => string | null, template?: (dir: string[]) => string | null}} ctx
 *   `currentBranch` gets the `cd`/`-C` directories in order, relative to the session's directory. `readFile` reads a
 *   PR description file and `template` the PR template, for `gh pr create` and `gh pr edit`; without them, a
 *   description file is refused as unreadable.
 * @returns {{deny: false} | {deny: true, reason: string}}
 */
export function checkCommand(command, ctx) {
  const r = checkText(command, ctx.shell, { ...ctx, dir: [] }, 0);
  if (!r) return { deny: false };
  return {
    deny: true,
    reason: `Blocked by the agent guard (scripts/agent-guard.mjs, #98): ${r.why}. ${r.advice}. Command: ${command}`,
  };
}
