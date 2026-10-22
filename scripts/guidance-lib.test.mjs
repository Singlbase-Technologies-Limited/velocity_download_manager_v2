import { describe, expect, it } from 'vitest';
import { checkGuidance, globToRegExp } from './guidance-lib.mjs';

const AGENTS = '# Rules\n\n- **Tests:** run `pnpm check:fast` before a pull request.\n';
const CLAUDE = '# Claude Code instructions\n\n@AGENTS.md\n\nRun `pnpm test:focused -- <area>` while iterating.\n';
const RULE = '---\npaths:\n  - "src/**/*.{ts,tsx}"\n---\n\n# Frontend\n\n- Keep components small.\n';
const SKILL = '---\nname: tdd-change\ndescription: Implement one behaviour test-first.\nargument-hint: "<behaviour>"\ndisable-model-invocation: true\n---\n\n# TDD\n\nRun `pnpm test:focused -- scripts`.\n';
const TRACKED = ['AGENTS.md', 'CLAUDE.md', 'package.json', 'src/App.tsx', 'src/main.tsx', 'scripts/a.mjs'];
const SCRIPTS = ['check:fast', 'test:focused', 'test', 'dev'];

/** A healthy guidance set; each test changes one file. */
function input(overrides = {}) {
  return {
    files: {
      'AGENTS.md': AGENTS,
      'CLAUDE.md': CLAUDE,
      '.claude/rules/frontend.md': RULE,
      '.claude/skills/tdd-change/SKILL.md': SKILL,
      ...overrides.files,
    },
    tracked: overrides.tracked ?? TRACKED,
    scripts: overrides.scripts ?? SCRIPTS,
  };
}
const problems = (overrides) => checkGuidance(input(overrides));

describe('checkGuidance', () => {
  it('accepts a healthy guidance set', () => {
    expect(problems()).toEqual([]);
  });

  describe('CLAUDE.md and AGENTS.md', () => {
    it('reports a CLAUDE.md of 200 lines or more', () => {
      expect(problems({ files: { 'CLAUDE.md': `${CLAUDE}${'- line\n'.repeat(200)}` } })).toEqual([
        expect.stringMatching(/^CLAUDE\.md: \d+ lines; keep it under 200/),
      ]);
    });
    it('reports an AGENTS.md of 200 lines or more, since CLAUDE.md imports it', () => {
      expect(problems({ files: { 'AGENTS.md': `${AGENTS}${'- line\n'.repeat(200)}` } })[0]).toMatch(/^AGENTS\.md: \d+ lines/);
    });
    it('reports a CLAUDE.md that does not import AGENTS.md', () => {
      expect(problems({ files: { 'CLAUDE.md': CLAUDE.replace('@AGENTS.md\n', '') } })).toEqual([
        'CLAUDE.md: it must import AGENTS.md with a line `@AGENTS.md`',
      ]);
    });
  });

  describe('path-scoped rules', () => {
    it('reports a rule without paths frontmatter', () => {
      expect(problems({ files: { '.claude/rules/frontend.md': '# Frontend\n\n- Keep components small.\n' } })).toEqual([
        '.claude/rules/frontend.md: no `paths:` frontmatter, so it would load in every session',
      ]);
    });
    it('reports a glob that matches no file (a dead glob)', () => {
      const rule = RULE.replace('src/**/*.{ts,tsx}', 'source/**/*.ts');
      expect(problems({ files: { '.claude/rules/frontend.md': rule } })).toEqual([
        '.claude/rules/frontend.md: the glob "source/**/*.ts" matches no file; fix it, or mark it `# pending: "source/**/*.ts" (#N)`',
      ]);
    });
    it('accepts a dead glob marked pending with its issue', () => {
      const rule = '---\n# pending: "browser-extension/**/*" (#64)\npaths:\n  - "browser-extension/**/*"\n---\n\n# Browser\n';
      expect(problems({ files: { '.claude/rules/frontend.md': rule } })).toEqual([]);
    });
    it('reports a pending note on a glob that now matches files', () => {
      const rule = RULE.replace('---\npaths:', '---\n# pending: "src/**/*.{ts,tsx}" (#29)\npaths:');
      expect(problems({ files: { '.claude/rules/frontend.md': rule } })).toEqual([
        '.claude/rules/frontend.md: the glob "src/**/*.{ts,tsx}" is marked pending but matches files now; remove the note',
      ]);
    });
    it('reports a pending note without an issue number', () => {
      const rule = '---\n# pending: "browser-extension/**/*"\npaths:\n  - "browser-extension/**/*"\n---\n';
      expect(problems({ files: { '.claude/rules/frontend.md': rule } })[0]).toMatch(/pending note must name its issue/);
    });
  });

  describe('skills', () => {
    it('reports a misspelled frontmatter field, which Claude Code would ignore silently', () => {
      const skill = SKILL.replace('argument-hint:', 'argument_hint:');
      expect(problems({ files: { '.claude/skills/tdd-change/SKILL.md': skill } })).toEqual([
        '.claude/skills/tdd-change/SKILL.md: unknown frontmatter field "argument_hint" (Claude Code ignores it silently)',
      ]);
    });
    it('reports a skill without frontmatter or a description', () => {
      expect(problems({ files: { '.claude/skills/tdd-change/SKILL.md': '# TDD\n' } })).toEqual([
        '.claude/skills/tdd-change/SKILL.md: no frontmatter with a `description:`',
      ]);
    });
    it('reports a SKILL.md of 500 lines or more', () => {
      expect(problems({ files: { '.claude/skills/tdd-change/SKILL.md': `${SKILL}${'- step\n'.repeat(500)}` } })[0]).toMatch(/\d+ lines; keep it under 500/);
    });
    it('reports a description that the skill listing would truncate', () => {
      const skill = SKILL.replace('Implement one behaviour test-first.', 'x'.repeat(1537));
      expect(problems({ files: { '.claude/skills/tdd-change/SKILL.md': skill } })[0]).toMatch(/1537 characters; the skill listing truncates it at 1536/);
    });
  });

  describe('pnpm commands', () => {
    it('reports a command that is neither a script nor a pnpm built-in', () => {
      const agents = `${AGENTS}- Run \`pnpm check:all\` first.\n`;
      expect(problems({ files: { 'AGENTS.md': agents } })).toEqual(['AGENTS.md: `pnpm check:all` is neither a package.json script nor a pnpm command']);
    });
    it('accepts pnpm built-ins and scripts in code spans and fenced blocks', () => {
      const agents = `${AGENTS}\n\`\`\`sh\npnpm install --frozen-lockfile\npnpm dev\n\`\`\`\n`;
      expect(problems({ files: { 'AGENTS.md': agents } })).toEqual([]);
    });
    it('ignores the word pnpm in prose', () => {
      expect(problems({ files: { 'AGENTS.md': `${AGENTS}- Use pnpm only.\n` } })).toEqual([]);
    });
    it('reports a package.json script that a pnpm built-in would shadow', () => {
      expect(problems({ scripts: [...SCRIPTS, 'doctor', 'setup'] })).toEqual([
        'package.json: the script "doctor" never runs, because `pnpm doctor` is a pnpm command; rename it',
        'package.json: the script "setup" never runs, because `pnpm setup` is a pnpm command; rename it',
      ]);
    });
    it('accepts scripts that pnpm runs in place of its built-in, such as test', () => {
      expect(problems({ scripts: [...SCRIPTS, 'start', 'clean'] })).toEqual([]);
    });
  });

  describe('forbidden terms', () => {
    it('reports a develop or staging branch', () => {
      const agents = `${AGENTS}- Open the pull request into \`develop\`.\n`;
      expect(problems({ files: { 'AGENTS.md': agents } })).toEqual([
        'AGENTS.md:4: mentions a develop or staging branch, which V2 doesn\'t have ("- Open the pull request into `develop`.")',
      ]);
    });
    it('reports aria2 use', () => {
      const rule = `${RULE}- Fall back to aria2c when libcurl fails.\n`;
      expect(problems({ files: { '.claude/rules/frontend.md': rule } })[0]).toMatch(/^\.claude\/rules\/frontend\.md:\d+: mentions aria2/);
    });
    it('accepts lines that rule them out', () => {
      const agents = `${AGENTS}- **No aria2:** V2 never calls aria2.\n- There is no \`develop\` or \`staging\` branch.\n- Use V1 only as a reference; don't copy aria2 code.\n`;
      expect(problems({ files: { 'AGENTS.md': agents } })).toEqual([]);
    });
  });

  describe('duplicates', () => {
    it('reports a requirement sentence repeated in two guidance files', () => {
      const sentence = '- Never pass user input through a shell; run programs with argument lists only.\n';
      const out = problems({ files: { 'AGENTS.md': AGENTS + sentence, '.claude/rules/frontend.md': RULE + sentence } });
      expect(out).toEqual([expect.stringMatching(/^repeated in \.claude\/rules\/frontend\.md and AGENTS\.md: "never pass user input/)]);
    });
    it('ignores short repeated lines such as headings', () => {
      expect(problems({ files: { 'AGENTS.md': `${AGENTS}## Tests\n`, '.claude/rules/frontend.md': `${RULE}## Tests\n` } })).toEqual([]);
    });
  });
});

describe('globToRegExp', () => {
  it.each([
    ['src/**/*.{ts,tsx}', 'src/App.tsx', true],
    ['src/**/*.{ts,tsx}', 'src/a/b/c.ts', true],
    ['src/**/*.{ts,tsx}', 'src/a.css', false],
    ['*.md', 'README.md', true],
    ['*.md', 'docs/README.md', false],
    ['docs/**/*.md', 'docs/decisions/0001.md', true],
    ['crates/*/tests/**/*', 'crates/velocity-curl/tests/linked_libcurl.rs', true],
    ['crates/*/tests/**/*', 'crates/velocity-curl/src/lib.rs', false],
    ['**/*.test.{ts,tsx,mjs}', 'scripts/guidance-lib.test.mjs', true],
    ['**/*.test.{ts,tsx,mjs}', 'top.test.ts', true],
    ['crates/*/src/**/checkpoint*.rs', 'crates/velocity-core/src/checkpoint.rs', true],
    ['package.json', 'package.json', true],
    ['package.json', 'src/package.json', false],
  ])('%s matches %s: %s', (glob, path, expected) => {
    expect(globToRegExp(glob).test(path)).toBe(expected);
  });
});
