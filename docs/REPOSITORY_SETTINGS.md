# Repository settings

The GitHub settings for this repository: each one's value, why, whether the current plan offers it, and a read-only command that shows it (#7). **Only the PO changes settings.** Agents read them and report.

## Where things stand

- **Owner and visibility:** since 2026-10-31 the repository is **public** and owned by the organisation **Singlbase-Technologies-Limited** (transferred from `vincentarisawork`; old links redirect). Public repositories get rulesets, environments with required reviewers, and the security features below on every plan, and their Actions minutes are free. The organisation's own policies can override a repository setting; where one does, the setting is greyed out with a note.
- **Applied by the PO on 2026-10-31,** in the five phases of #90's checklist, and checked from screenshots and the API; earlier checks on 2026-10-30 with the commands below. The agent account has Write access, not Admin, so the commands marked *Admin* only work with the PO's account. They read and change nothing.
- **State column:** *On* means the setting has the recommended value. *Recommended* means the PO hasn't applied it yet. *Not yet* means it waits for something named in the row.

Set `R=repos/Singlbase-Technologies-Limited/velocity_download_manager_v2` before running the commands.

## Merging

| Setting | Value | State | Why | Check |
|---|---|---|---|---|
| Allow squash merging | On; merge commits and rebase merging off | On | One commit per issue on `main` ([DEVELOPMENT_WORKFLOW.md](DEVELOPMENT_WORKFLOW.md)) | `gh api $R --jq '{allow_squash_merge, allow_merge_commit, allow_rebase_merge}'` → `true`, `false`, `false` |
| Automatically delete head branches | On | On | Branches are deleted after merge | `gh api $R --jq .delete_branch_on_merge` → `true` |
| Allow auto-merge | Off | On | The PO decides each merge | `gh api $R --jq .allow_auto_merge` → `false` |

Source: [configuring pull request merges](https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/configuring-pull-request-merges).

## Rules for the main branch

A ruleset named **Main** targets the default branch, is *Active*, and has no bypass list (created by the PO on 2026-10-30). Rulesets are in Settings → Rules → Rulesets ([about rulesets](https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/managing-rulesets/about-rulesets), [available rules](https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/managing-rulesets/available-rules-for-rulesets)).

| Rule | Value | State | Why |
|---|---|---|---|
| Restrict deletions | On | On | `main` can't be deleted |
| Block force pushes | On | On | `main`'s history is never rewritten |
| Require a pull request before merging | On, 0 approvals | On (2026-10-31) | Stops a direct push to `main` on the server, not only in the agent's guard ([CLAUDE.md](../CLAUDE.md#instructions-versus-enforcement)). With 0 approvals, the PO can still merge their own PRs; GitHub never lets an author approve their own PR |
| Require linear history | On | On (2026-10-31) | Matches squash-only merging; refuses a merge commit pushed by mistake |
| Require status checks to pass (`ci-required`) | On | Not yet: CI runs by hand only (#204) | PR checks don't start on a pull request now, so a required check would stay pending and block every merge. Turn it on when the automatic triggers come back ([CI.md](CI.md)) |
| Require conversation resolution | On | Optional | Every review comment answered before merging |
| Bypass list | Empty | On | Nobody, the PO included, skips the rules by accident. The PO can still change the ruleset |
| Tag rule for `v*` | Only the PO creates, updates or deletes release tags | On (2026-10-31): the **Release Tags** ruleset restricts creations, updates and deletions and blocks force pushes on `v*`; Organization admin and Repository admin may bypass, which means the PO. The agent can't create a release tag | Releases are tags on verified `main` commits |
| CODEOWNERS enforcement | Off | Not yet: one maintainer | Needs a second reviewer to mean anything ([DEVELOPMENT_WORKFLOW.md](DEVELOPMENT_WORKFLOW.md)) |

Check, with the agent's account or the PO's:

```sh
gh api $R/rulesets --jq '.[] | {name, enforcement}'   # → Main and Release Tags, both "active"
gh api $R/rules/branches/main --jq '[.[].type]'       # → ["deletion","non_fast_forward","required_linear_history","pull_request"]
```

## Actions

| Setting | Value | State | Why | Check (*Admin*) |
|---|---|---|---|---|
| Workflow permissions | Read repository contents | On | Each job asks for any write access it needs in its own `permissions:` ([CI.md](CI.md#permissions-and-supply-chain)) | `gh api $R/actions/permissions/workflow` → `"default_workflow_permissions":"read"` |
| Allow Actions to create and approve pull requests | Off | On | No workflow approves its own changes | the same command → `"can_approve_pull_request_reviews":false` |
| Allowed actions | Allow actions created by GitHub only | On (2026-10-31): Singlbase's and GitHub's actions; Marketplace creators off | The workflows use only `actions/checkout`, `actions/setup-node`, `actions/cache`, `actions/upload-artifact` and `actions/download-artifact`, plus the repository's own composite actions. Narrowing the list stops a third-party action from being added without a settings change | `gh api $R/actions/permissions` → `"allowed_actions":"selected"`, and `gh api $R/actions/permissions/selected-actions` → `"github_owned_allowed":true` |
| Require actions to be pinned to a full-length commit SHA | On | On (2026-10-31) | Every action is already pinned to a SHA ([DEPENDENCIES.md](DEPENDENCIES.md)); this makes GitHub refuse one that isn't | `gh api $R/actions/permissions` → `"sha_pinning_required":true` |
| Approval for fork pull request workflows | Require approval for first-time contributors | On (2026-10-31) | A stranger's first pull request runs no workflow until a maintainer approves it ([OPEN_SOURCE.md](OPEN_SOURCE.md)) | Settings → Actions → General |

Source: [managing GitHub Actions settings for a repository](https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/enabling-features-for-your-repository/managing-github-actions-settings-for-a-repository).

## The release environment

| Setting | Value | State | Why |
|---|---|---|---|
| Environment `release` | Required reviewer: the PO, self-review allowed; administrators can't bypass; deployments only from `v*` tags | On (2026-10-31) | The release workflow's `draft` job waits here for the PO's approval ([RELEASE_PROCESS.md](RELEASE_PROCESS.md#the-release-environment)) |

Check: `gh api $R/environments/release --jq '{can_admins_bypass, rules: [.protection_rules[].type]}'` → `false`, `["required_reviewers","branch_policy"]`.

## Security

| Setting | Value | Plan | State | Check (*Admin*) |
|---|---|---|---|---|
| Dependabot alerts | On | Every plan | On | `gh api -i $R/vulnerability-alerts` → `HTTP/2.0 204` |
| Dependabot security updates | On (PO decision on #75) | Every plan | On | `gh api $R/automated-security-fixes` → `{"enabled":true,"paused":false}` |
| Dependabot version updates | GitHub Actions only, monthly, grouped | Every plan | On, in [`.github/dependabot.yml`](../.github/dependabot.yml) | the file itself |
| Secret scanning and push protection | On, with validity checks; only admins may bypass push protection | Public repositories; private ones need GitHub Secret Protection (Team or Enterprise) | On (2026-10-31) | `gh api $R --jq .security_and_analysis` |
| Code scanning and dependency review | On: CodeQL default setup, with Copilot Autofix | Public repositories; private ones need GitHub Code Security (Team or Enterprise) | On (2026-10-31). Its first scan's alerts are #233 | Settings → Code security |
| Private vulnerability reporting | On | Public repositories | On (2026-10-31) |
| Dependabot malware alerts | On | Public repositories | On (2026-10-31) | `gh api $R/private-vulnerability-reporting` |

Source: [GitHub security features](https://docs.github.com/en/code-security/getting-started/github-security-features). What V2 does with an advisory is in [SECURITY.md](SECURITY.md#dependency-advisories-75).

## When something changes

- **The PO changes a setting:** update its row and the date checked, and note the issue or decision.
- **CI's automatic triggers come back:** make `ci-required` a required check in the Main ruleset.
- **The repository goes public:** apply the rows marked *at publication*, with the rest of #90's checklist.
