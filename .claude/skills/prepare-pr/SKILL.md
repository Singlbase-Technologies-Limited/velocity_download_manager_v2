---
name: prepare-pr
description: Pre-pull-request checklist for V2. Checks scope against the issue, the evidence for every acceptance criterion, docs, commit and title format, closing keywords and the agent identity, then drafts the PR description. It does not push or open the PR without the user's go-ahead. Manual.
disable-model-invocation: true
argument-hint: "<issue number>"
---

# Prepare the pull request for #$ARGUMENTS

Check every item. Report anything that fails, and fix it or ask; don't skip it. What the reviewer checks is in [docs/REVIEWING.md](../../../docs/REVIEWING.md).

## 1. Scope and state

- **Clean branch from current `main`:** `git status` is clean or fully committed. The branch is `<type>/$ARGUMENTS-<slug>` and is based on the current `origin/main` (`git merge-base --is-ancestor origin/main HEAD`).
- **Scope:** `git diff --stat origin/main...HEAD` touches only what issue #$ARGUMENTS needs. Anything else is split out or gets its own issue.
- **No stray files:** no secrets, logs, build output or binaries are committed. Logs stay in the scratch directory.

## 2. Evidence

- **Each acceptance criterion** has its evidence: a command and its real output, a transcript, or a file and line.
- **Red first:** for meaningful behaviour, the failing test output was recorded (`/tdd-change`).
- **Verification ran** (`/verify-v2`). Every check is PASS, FAIL, BLOCKED or NOT RUN, per platform.
- **No test** was deleted, skipped or weakened, and no lint was relaxed.

## 3. Documentation

- **The issue's "Documentation impact"** is done in this branch. New docs are in `docs/README.md`, and new terms in `docs/GLOSSARY.md`.
- **The root README** was reviewed.
- **Links:** relative links and anchors in the changed Markdown files resolve.

## 4. Title, commit and description

- **Conventional Commit title.** A single-commit PR keeps that commit's message on `main`, so it must follow the convention too ([docs/DEVELOPMENT_WORKFLOW.md](../../../docs/DEVELOPMENT_WORKFLOW.md)).
- **Commit trailer:** the commit message ends with the model's `Co-Authored-By:` line.
- **Closing keywords:** the first line is `Closes #$ARGUMENTS.` only if merging completes every acceptance criterion; otherwise `Part of #$ARGUMENTS.` Search the whole description for "close", "fix" and "resolve" followed by `#N`: GitHub acts on those even inside a sentence.
- **The description is the PR template, filled in** (PO decision on #59). Copy [`.github/PULL_REQUEST_TEMPLATE.md`](../../../.github/PULL_REQUEST_TEMPLATE.md) to a file in the scratch directory and fill it in. Never copy an earlier PR's shape: several drifted from the template.
  - Keep every section, in its order, and add none.
  - Keep each checklist item in the template's words, ticked or not; add detail after it.
  - Fill every table: each result is PASS, FAIL, BLOCKED or NOT RUN.
  - Fill "**Not included:**", the commands block and the verification level.
  - Remove every comment.
  - Keep the reviewer's footer, then end with the Claude Code attribution line.
- **Title:** `<type>(<scope>): <description> (#N)`, for example `feat(engine): add bounded range probe (#41)`.
- **Check it:** `pnpm check:pr-body <file> --title "<title>"` must pass. The agent guard runs the same check, and refuses `gh pr create` or `gh pr edit` without `--body-file <literal path>`, or with a description that fails it.

## 5. Identity, then stop

Run the four checks in [docs/AI_AGENT_ACCOUNT.md](../../../docs/AI_AGENT_ACCOUNT.md#check-before-you-push-or-open-a-pr): remote, email, SSH and `gh api user`.

**Then show the user the drafted title and description and wait for the go-ahead** before `git push` and `gh pr create`. After opening:

- Confirm `closingIssuesReferences` is what you intended.
- Set the board item to *In review*.
