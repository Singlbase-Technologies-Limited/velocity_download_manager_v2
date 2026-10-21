# Contributing

Thanks for helping with Velocity Download Manager V2. No licence has been chosen yet (#86), so contributions can't be accepted under clear terms. Please open an issue to discuss before you start.

This page is the path from a fresh clone to a merged pull request. The rules behind each step are in [AGENTS.md](AGENTS.md), which applies to people and AI agents alike.

## 1. Set up

Install the prerequisites, clone and run the app by following [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md):

```sh
pnpm check:prereqs
pnpm install --frozen-lockfile
pnpm dev
```

If something fails, see [docs/TROUBLESHOOTING.md](docs/TROUBLESHOOTING.md).

## 2. Learn the code

- **[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md):** the components, who owns which state, and the open decisions.
- **[docs/GLOSSARY.md](docs/GLOSSARY.md):** the terms the code and issues use.
- **[docs/CONVENTIONS.md](docs/CONVENTIONS.md):** naming.
- **[docs/decisions/](docs/decisions/README.md):** decisions already taken. Don't reopen one without new evidence.

## 3. Pick an issue

Work is planned as issues on the [project board](https://github.com/users/vincentarisawork/projects/2).

1. **Choose an issue** in *Ready* whose dependencies are closed.
2. **Read the issue in full,** including its acceptance criteria, non-goals and comments.
3. **Search the existing code** for anything that already does part of the job.

## 4. Branch, test, build

1. **Branch from the latest `main`** as `<type>/<issue>-<slug>`, following [docs/DEVELOPMENT_WORKFLOW.md](docs/DEVELOPMENT_WORKFLOW.md).
2. **Write a failing test first,** make it pass, then refactor ([docs/TESTING.md](docs/TESTING.md)). Iterate with `pnpm test:focused -- <area>`.
3. **Update the docs** that describe what you changed, in the same branch.
4. **Run `pnpm check:fast`.** It must pass with zero warnings.

## 5. Open a pull request

- **Target `main`.** Use a Conventional Commit title, such as `feat(engine): add bounded range probe`, and link the issue.
- **Describe the change:** what changed, the commands you ran with their results, and every check marked PASS, FAIL, BLOCKED or NOT RUN.
- **Review:** the Product Owner reviews against [docs/REVIEWING.md](docs/REVIEWING.md) and decides the merge.
