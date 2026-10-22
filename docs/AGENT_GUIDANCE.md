# Agent guidance

How V2's instructions for contributors and AI agents are organised, checked and reviewed (#88). The rules themselves are in [AGENTS.md](../AGENTS.md).

## Where guidance lives

| File | Holds | Loaded by Claude Code |
|---|---|---|
| [AGENTS.md](../AGENTS.md) | The rules for every contributor, human or agent | In every session, through CLAUDE.md's `@AGENTS.md` import |
| [CLAUDE.md](../CLAUDE.md) | How Claude Code works through an issue, the skills, and the rule map | In every session |
| `.claude/rules/*.md` | Rules for one area, such as Rust or the browser extension, with `paths:` globs | When Claude reads or edits a file that matches a glob |
| `.claude/skills/*/SKILL.md` | One step of the workflow each, such as `/investigate-issue` or `/prepare-pr` | Their descriptions in every session; the body when invoked |
| `.claude/settings.json` | Deny and ask rules, and the agent guard hook ([CLAUDE.md](../CLAUDE.md#instructions-versus-enforcement)) | Enforced, not guidance |

Instructions are guidance: Claude Code applies them, but doesn't enforce them. A rule that must hold no matter what goes into a hook, as the agent guard does for pushes to `main` (#98).

## What `pnpm check:guidance` checks

[`scripts/check-guidance.mjs`](../scripts/check-guidance.mjs) reads the files above; the checks are in [`guidance-lib.mjs`](../scripts/guidance-lib.mjs), each with a failing fixture in its tests. It takes under 100 ms.

| Check | Why |
|---|---|
| CLAUDE.md and AGENTS.md each under 200 lines | Claude Code's guidance: longer files cost context and reduce adherence, and an import loads in full like the file itself ([memory docs](https://code.claude.com/docs/en/memory)) |
| CLAUDE.md imports AGENTS.md (`@AGENTS.md`) | Otherwise the shared rules never reach Claude |
| Every rule has `paths:`, and each glob matches a file or is marked pending | A rule without `paths:` loads in every session; a dead glob never loads at all |
| Every skill has a `description:`, only known frontmatter fields, under 500 lines, and a listing under 1,536 characters | Claude Code ignores an unknown field silently, and truncates a long description ([skills docs](https://code.claude.com/docs/en/skills)) |
| Every `pnpm <command>` in code is a `package.json` script or a pnpm command | A renamed script leaves instructions that fail |
| No `package.json` script is named like a pnpm command, such as `doctor` or `setup` | `pnpm doctor` runs pnpm's own command, so the script never runs |
| No `develop` or `staging` branch, and no aria2 use | V2 has neither ([DEVELOPMENT_WORKFLOW.md](DEVELOPMENT_WORKFLOW.md), [ADR-0001](decisions/0001-v2-direction.md)); a line that rules them out ("no", "never", V1 as a reference) passes |
| No requirement sentence repeated in two guidance files | Copies drift apart; keep one and link to it (AGENTS.md, "Documentation") |

Relative links in these files are checked by `pnpm check:links`, like every other Markdown file.

**A glob for a folder that doesn't exist yet** is marked in the rule's frontmatter with the issue that creates it:

```yaml
---
# pending: "browser-extension/**/*" (#64)
paths:
  - "browser-extension/**/*"
---
```

The check fails when a pending glob starts matching files, so the note is removed when its issue lands.

**When it runs:**

- **Locally:** `pnpm check:guidance`.
- **In CI:** on every pull request, in the `select` job, and after every merge to `main` ([CI.md](CI.md)). It runs whatever changed, because any path can change its result: deleting a folder can kill a rule's glob.
- **In Vitest:** `scripts/check-guidance.test.mjs` runs it on the real files, so `pnpm check:fast` catches drift too.

**Keeping it current:** the skill fields and pnpm commands are lists in `guidance-lib.mjs`, taken from the Claude Code docs and `pnpm --help` of the pinned pnpm. Update them when Claude Code or pnpm changes.

## The periodic prompt audit

Claude Code's `/doctor prompt-audit` reads the guidance for problems no structural check finds: instructions written for older models, statements that are no longer true, and files that contradict each other ([memory docs](https://code.claude.com/docs/en/memory#audit-your-instruction-files)). It proposes edits and changes nothing on its own. It needs Claude Code 2.1.283 or later.

- **When:** monthly, and after every Claude Code upgrade.
- **How:** run `claude -p "/doctor prompt-audit"` from PowerShell in the repository (Git Bash rewrites the leading `/`; CLAUDE.md, "Windows shells"), or `/doctor prompt-audit` in a session.
- **Record:** open an issue with the findings, and fix them through the normal workflow.

| Date | Claude Code | Result | Issue |
|---|---|---|---|
| 2026-10-22 | 2.1.308 | No old-model patterns. 4 out-of-date statements and 2 wording conflicts in skills, rules and CLAUDE.md. The rule comments it flagged were replaced by pending notes in #88, and #130 fixed the other four (finding 6 kept, PO decision) | #130 |

In `claude -p`, the audit couldn't read `~/.claude/` or the bundled audit checklist (no permission prompt in that mode), so the user-level files were not audited.
