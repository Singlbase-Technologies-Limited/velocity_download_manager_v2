# Open source

How V2 is run as an open-source project, and the checklist for making the repository public (#90). The licence is GPL-3.0-only, copyright Singlbase Technologies Limited ([ADR-0017](decisions/0017-gpl-3-licence.md)).

## Governance

- **Owner:** Singlbase Technologies Limited owns V2 and its name.
- **Decisions:** the Product Owner decides scope, architecture, dependencies, licensing, security trade-offs, repository settings, merges and releases ([AGENTS.md](../AGENTS.md#project-and-authority)). Accepted decisions are in [decisions/](decisions/README.md).
- **Maintainers:** the Product Owner (`vincentarisawork`), and the machine account `velocity-claude-agent` for AI-agent work, which never approves, releases or changes settings ([AI_AGENT_ACCOUNT.md](AI_AGENT_ACCOUNT.md)).
- **Contributions:** anyone may propose a change. Outside contributors agree to the [CLA](../CLA.md) once, on their first pull request ([CONTRIBUTING.md](../CONTRIBUTING.md#licence-and-the-contributor-licence-agreement)).

## The community files

| File | What |
|---|---|
| [`LICENSE`](../LICENSE) | GPL-3.0, the full text |
| [`CLA.md`](../CLA.md) | The Contributor Licence Agreement (a draft until reviewed by a lawyer) |
| [`CONTRIBUTING.md`](../CONTRIBUTING.md) | From clone to merged pull request, and the CLA |
| [`CODE_OF_CONDUCT.md`](../CODE_OF_CONDUCT.md) | The Contributor Covenant, version 2.1 |
| [`SECURITY.md`](../SECURITY.md) | Private vulnerability reporting, supported versions |
| [`SUPPORT.md`](../SUPPORT.md) | Where to get help |

## Before going public

The audit run for #90 on 2026-10-31, on every branch's history (102 commits) and every issue, pull request and comment:

| Check | How | Result |
|---|---|---|
| Secrets in the git history | `git log --all -p`, searched for private keys, GitHub tokens (`ghp_`, `github_pat_`, `gho_`), AWS keys (`AKIA…`), Slack tokens (`xox…`), API keys (`sk-…`, `AIza…`) and long bearer tokens | **None.** The only private-key markers are three made-up keys in the redaction tests (`crates/velocity-core/src/redact.rs`, `diagnostics.rs`) |
| Secret files ever committed | Every file name in the history, for `.pem`, `.key`, `.p12`, `.pfx`, `id_rsa`, `id_ed25519` and `.env` | **None.** The Chrome extension's private key was never committed (`*.pem` is ignored) |
| Secrets in issues, pull requests and comments | The same patterns, and `password=` | **None** |
| Personal data in files | Personal names, home-folder paths | A Windows user name in a test (`paths.rs`) was replaced with `Jane Doe`; the history keeps the old commits |
| Email addresses in commits | `git log --all --format='%ae %ce'` | **Decision for the PO:** 96 commits by `vincentarisawork+velocity-agent@gmail.com` and 4 by `admin@singlbase.com` become public with the repository. Removing them means rewriting history, which the rules forbid without the PO's decision |
| Workflows a fork's pull request could abuse | `.github/workflows/*.yml` | **Safe:** no `pull_request_target`, no secrets used anywhere, every token read-only unless a job needs more, every action pinned to a commit |
| Third-party notices | `pnpm check:notices` (#76) | Complete: every component V2 ships, with its licence |
| Test credentials | The SFTP test servers | For the tests only: a throwaway user with a random password, created on the CI runner and masked in its log (`scripts/ci-sftp-server.sh`), never a real account |

### The checklist

The PO does these, in this order, and ticks each in #90:

1. **Merge #90** and everything before it, so `main` has the licence and these files.
2. **Decide on the commit emails** above.
3. **Make the repository public:** Settings, General, Danger Zone, **Change visibility**.
4. **Turn on, in Settings, Advanced Security** (free for public repositories): **Private vulnerability reporting** (`SECURITY.md` relies on it), **Dependabot alerts**, **Secret scanning** and **Push protection**, **Code scanning** with the default CodeQL setup. *Check:* the Security tab shows "Report a vulnerability".
5. **Actions, General:** "Require approval for first-time contributors" for fork pull requests; workflow permissions "Read repository contents". *Check:* a fork's pull request waits for approval before any workflow runs.
6. **Branch rules for `main`** as in [REPOSITORY_SETTINGS.md](REPOSITORY_SETTINGS.md): a pull request, linear history, no force push.
7. **Turn the pull-request checks back on,** now that minutes are free: CI runs by hand only today (#204). *Check:* a pull request starts PR checks.
8. **Optional:** an [OpenSSF Scorecard](https://github.com/ossf/scorecard) run, as an outside check of the repository's practices.

## After going public

- **The deferred testing** (PO decision, 2026-10-31): every check not run since then, the container install tests (#82), the first release candidate (#17), the parity check (#85), the stress and fuzz tests (#77, #73), and the release-readiness review (#78).
- **The CLA's legal review,** before the first outside contribution is merged.
