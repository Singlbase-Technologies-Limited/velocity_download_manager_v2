# Claude Code instructions

@AGENTS.md

## Operating procedure (bootstrap version; issue #4 expands it)

1. Read the issue, its dependencies and the linked documents. Check the live state before acting: `git status`, the current branch, open pull requests.
2. For work that touches several files or changes the architecture, write a short plan before editing.
3. Work on the issue's branch. Never commit to `main`, force-push, or rewrite shared history.
4. Follow Red–Green–Refactor. Run focused tests while working and `pnpm check:fast` before the pull request.
5. If something fails, stop and report the actual error. Don't work around it by weakening tests or checks.
6. Ask the Product Owner before any decision on scope, architecture, dependencies, licensing, security, repository settings, merges or releases.
7. Finish with a report: the files changed, the commands run with their results, documentation updated, and anything NOT RUN or BLOCKED.
