// Pure checks for the agent guidance files (#88, docs/AGENT_GUIDANCE.md): AGENTS.md, CLAUDE.md,
// .claude/rules/*.md and .claude/skills/*/SKILL.md. Claude Code treats these as guidance and
// fails silently on many mistakes (an unknown skill field is ignored, a dead rule glob never
// loads), so the structure is checked here. scripts/check-guidance.mjs reads the files.
//
// Sources: Claude Code's memory docs (https://code.claude.com/docs/en/memory: under 200 lines per
// CLAUDE.md, imports count; rules' `paths:` globs) and skills docs
// (https://code.claude.com/docs/en/skills: the frontmatter fields, under 500 lines, a 1,536-character
// listing limit). Update the lists below with those docs.

/** Every SKILL.md frontmatter field Claude Code reads (skills docs, frontmatter reference). */
export const SKILL_FIELDS = new Set([
  'name',
  'description',
  'when_to_use',
  'argument-hint',
  'arguments',
  'disable-model-invocation',
  'user-invocable',
  'allowed-tools',
  'disallowed-tools',
  'model',
  'effort',
  'context',
  'agent',
  'background',
  'hooks',
  'paths',
  'shell',
  'metadata',
  'license',
  'compatibility',
]);

/** Commands and aliases of the pinned pnpm (12.14.0, `pnpm --help`, 2026-10-22). */
export const PNPM_COMMANDS = new Set(
  (
    'access init recursive multi m add install i install-test it update up upgrade outdated audit change version lane bugs ' +
    'issues list ls ll la licenses licences why view info show v sbom whoami deprecate undeprecate unpublish star unstar ' +
    'stars dist-tag dist-tags ping doctor search s se find rebuild rb pack publish stage remove uninstall rm un uni patch ' +
    'patch-commit patch-remove peers set-script ss test t tst run run-script tasks pipeline exec dlx create completion ' +
    'start stop restart find-hash runtime rt env shim bin clean purge ci clean-install ic install-clean root prefix config ' +
    'c get set pkg pack-app store cache cat-file cat-index ignored-builds approve-builds permissions approve link ln ' +
    'import dedupe deploy prune fetch unlink dislink docs home repo self-update setup login adduser team owner owners ' +
    'logout with edit profile token xmas help'
  ).split(' '),
);
/** pnpm commands that run a same-named package.json script instead of themselves (`pnpm --help`). */
const SCRIPT_FIRST = new Set(['test', 't', 'tst', 'start', 'stop', 'restart', 'tasks', 'clean', 'purge']);

const MAX_INSTRUCTION_LINES = 200;
const MAX_SKILL_LINES = 500;
const MAX_LISTING_CHARS = 1536;
const MIN_DUPLICATE_CHARS = 40;
const NEGATION = /\b(no|not|never|nor|without|don't|doesn't|isn't|aren't|won't)\b/i;

/** A `paths:`-style glob as an anchored RegExp: `**`, `*`, `?` and `{a,b}`. */
export function globToRegExp(glob) {
  let re = '';
  for (let i = 0; i < glob.length; i++) {
    const c = glob[i];
    if (c === '*' && glob[i + 1] === '*') {
      if (glob[i + 2] === '/') {
        re += '(?:.*/)?';
        i += 2;
      } else {
        re += '.*';
        i += 1;
      }
    } else if (c === '*') re += '[^/]*';
    else if (c === '?') re += '[^/]';
    else if (c === '{') {
      const end = glob.indexOf('}', i);
      re += `(?:${glob
        .slice(i + 1, end)
        .split(',')
        .map((p) => p.replace(/[.+^$()|[\]\\]/g, '\\$&'))
        .join('|')})`;
      i = end;
    } else re += c.replace(/[.+^$()|[\]\\]/g, '\\$&');
  }
  return new RegExp(`^${re}$`);
}

const lineCount = (text) => (text.endsWith('\n') ? text.split('\n').length - 1 : text.split('\n').length);

/** YAML frontmatter as `{fields, lists, comments}`, enough for the simple keys these files use; null without one. */
function frontmatter(text) {
  if (!text.startsWith('---\n') && !text.startsWith('---\r\n')) return null;
  const lines = text.split(/\r?\n/);
  const end = lines.indexOf('---', 1);
  if (end < 0) return null;
  const fields = {};
  const lists = {};
  const comments = [];
  let last = null;
  for (const line of lines.slice(1, end)) {
    if (/^\s*#/.test(line)) comments.push(line.trim());
    else if (/^\s+-\s+/.test(line) && last) (lists[last] ??= []).push(line.replace(/^\s+-\s+/, '').replace(/^"(.*)"$/, '$1'));
    else {
      const m = /^([A-Za-z0-9_-]+):\s*(.*)$/.exec(line);
      if (m) {
        last = m[1];
        fields[m[1]] = m[2].replace(/^"(.*)"$/, '$1').replace(/^'(.*)'$/, '$1');
      }
    }
  }
  return { fields, lists, comments };
}

function checkRule(path, text, tracked) {
  const fm = frontmatter(text);
  const globs = fm?.lists.paths ?? [];
  if (!globs.length) return [`${path}: no \`paths:\` frontmatter, so it would load in every session`];
  const problems = [];
  const pending = new Map();
  for (const c of fm.comments) {
    const m = /^#\s*pending:\s*"([^"]+)"\s*(\(#\d+\))?/i.exec(c);
    if (m) pending.set(m[1], Boolean(m[2]));
  }
  for (const glob of globs) {
    const re = globToRegExp(glob);
    const matches = tracked.some((f) => re.test(f));
    if (pending.has(glob)) {
      if (!pending.get(glob)) problems.push(`${path}: a pending note must name its issue: \`# pending: "${glob}" (#N)\``);
      else if (matches) problems.push(`${path}: the glob "${glob}" is marked pending but matches files now; remove the note`);
    } else if (!matches) {
      problems.push(`${path}: the glob "${glob}" matches no file; fix it, or mark it \`# pending: "${glob}" (#N)\``);
    }
  }
  return problems;
}

function checkSkill(path, text) {
  const fm = frontmatter(text);
  if (!fm || !fm.fields.description) return [`${path}: no frontmatter with a \`description:\``];
  const problems = [];
  for (const key of Object.keys(fm.fields)) {
    if (!SKILL_FIELDS.has(key)) problems.push(`${path}: unknown frontmatter field "${key}" (Claude Code ignores it silently)`);
  }
  const lines = lineCount(text);
  if (lines >= MAX_SKILL_LINES) problems.push(`${path}: ${lines} lines; keep it under ${MAX_SKILL_LINES} and move reference material into separate files`);
  const listing = fm.fields.description.length + (fm.fields.when_to_use ? fm.fields.when_to_use.length + 1 : 0);
  if (listing > MAX_LISTING_CHARS) problems.push(`${path}: the description is ${listing} characters; the skill listing truncates it at ${MAX_LISTING_CHARS}`);
  return problems;
}

/** The code in a Markdown file: inline code spans and fenced blocks, where commands are written. */
function codeOf(text) {
  const code = [];
  const prose = text.replace(/^```[^\n]*\n([\s\S]*?)^```/gm, (_, block) => {
    code.push(block);
    return '';
  });
  for (const m of prose.matchAll(/`([^`\n]+)`/g)) code.push(m[1]);
  return code;
}

/**
 * The command word after each `pnpm` in `code`, skipping its options: `pnpm -s test:focused -- x`
 * gives `test:focused`. A token walk, not one regular expression: a pattern with optional options
 * and the spaces around them backtracked over long runs of spaces (CodeQL js/redos, #233).
 */
export function pnpmWords(code) {
  const words = [];
  for (const m of code.matchAll(/(?:^|[\s;&|(])pnpm(?=\s)/g)) {
    const tokens = code.slice(m.index + m[0].length).trimStart().split(/\s+/);
    const command = tokens.find((t) => !t.startsWith('-'));
    const word = command === undefined ? null : /^[A-Za-z0-9][\w:.-]*/.exec(command)?.[0];
    if (word) words.push(word);
  }
  return words;
}

function checkPnpmCommands(path, text, scripts) {
  const problems = [];
  const seen = new Set();
  for (const code of codeOf(text)) {
    for (const word of pnpmWords(code)) {
      if (seen.has(word) || scripts.includes(word) || PNPM_COMMANDS.has(word)) continue;
      seen.add(word);
      problems.push(`${path}: \`pnpm ${word}\` is neither a package.json script nor a pnpm command`);
    }
  }
  return problems;
}

function checkForbidden(path, text) {
  const problems = [];
  text.split(/\r?\n/).forEach((line, i) => {
    const at = `${path}:${i + 1}`;
    const quote = line.trim().length > 120 ? `${line.trim().slice(0, 117)}...` : line.trim();
    if (/\b(develop|staging)\b/i.test(line) && !NEGATION.test(line)) {
      problems.push(`${at}: mentions a develop or staging branch, which V2 doesn't have ("${quote}")`);
    }
    if (/\baria2c?\b/i.test(line) && !NEGATION.test(line) && !/\b(V1|reference)\b/.test(line)) {
      problems.push(`${at}: mentions aria2, which V2 never bundles, calls or falls back to ("${quote}")`);
    }
  });
  return problems;
}

/** Requirement sentences, normalised, outside frontmatter and code. */
function sentences(text) {
  const body = text.replace(/^---\r?\n[\s\S]*?\r?\n---\r?\n/, '').replace(/^```[^\n]*\n[\s\S]*?^```/gm, '');
  return body
    .split(/\n|(?<=[.!?])\s+/)
    .map((s) =>
      s
        .replace(/^\s*(?:[-*]|\d+\.)\s+/, '')
        .replace(/[*_`#>]/g, '')
        .replace(/\s+/g, ' ')
        .trim()
        .toLowerCase(),
    )
    .filter((s) => s.length >= MIN_DUPLICATE_CHARS);
}

function findDuplicates(files) {
  const where = new Map();
  for (const [path, text] of Object.entries(files)) {
    for (const s of new Set(sentences(text))) {
      if (!where.has(s)) where.set(s, new Set());
      where.get(s).add(path);
    }
  }
  const problems = [];
  for (const [s, paths] of where) {
    if (paths.size > 1) {
      const quote = s.length > 90 ? `${s.slice(0, 87)}...` : s;
      problems.push(`repeated in ${[...paths].sort().join(' and ')}: "${quote}"; keep it in one file and link to it`);
    }
  }
  return problems;
}

/**
 * @param {{files: Record<string, string>, tracked: string[], scripts: string[]}} input
 *   `files`: the guidance files by repository-relative path; `tracked`: every file in the
 *   repository, for the rule globs; `scripts`: the package.json script names.
 * @returns {string[]} the problems found; empty when the guidance is in shape
 */
export function checkGuidance({ files, tracked, scripts }) {
  const problems = [];
  for (const [path, text] of Object.entries(files)) {
    if (path === 'CLAUDE.md' || path === 'AGENTS.md') {
      const lines = lineCount(text);
      if (lines >= MAX_INSTRUCTION_LINES) {
        problems.push(`${path}: ${lines} lines; keep it under ${MAX_INSTRUCTION_LINES} (Claude Code loads it, and every import, in full in every session)`);
      }
      if (path === 'CLAUDE.md' && !text.split(/\r?\n/).some((l) => l.trim() === '@AGENTS.md')) {
        problems.push('CLAUDE.md: it must import AGENTS.md with a line `@AGENTS.md`');
      }
    } else if (/^\.claude\/rules\/.+\.md$/.test(path)) problems.push(...checkRule(path, text, tracked));
    else if (/^\.claude\/skills\/[^/]+\/SKILL\.md$/.test(path)) problems.push(...checkSkill(path, text));
    problems.push(...checkPnpmCommands(path, text, scripts), ...checkForbidden(path, text));
  }
  for (const name of scripts) {
    if (PNPM_COMMANDS.has(name) && !SCRIPT_FIRST.has(name)) {
      problems.push(`package.json: the script "${name}" never runs, because \`pnpm ${name}\` is a pnpm command; rename it`);
    }
  }
  problems.push(...findDuplicates(files));
  return problems;
}
