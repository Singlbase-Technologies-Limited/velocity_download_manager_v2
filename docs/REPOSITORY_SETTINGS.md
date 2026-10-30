# Repository settings

The GitHub settings for this repository: each one's value, why, whether the current plan offers it, and a read-only command that shows it (#7). **Only the PO changes settings.** Agents read them and report.

## Where things stand

- **Plan:** the account is on GitHub Pro ([CI.md](CI.md#while-ci-runs-by-hand-only), #204). The repository is private. On GitHub Free a private repository has no rulesets or branch protection (the API returned HTTP 403 on 2026-10-21, #89); on Pro it has both ([GitHub's plans](https://docs.github.com/en/get-started/learning-about-github/githubs-plans)).
- **Checked on 2026-10-30** with the commands below. The agent account has Write access, not Admin, so the commands marked *Admin* only work with the PO's account. They read and change nothing.
- **State column:** *On* means the setting has the recommended value. *Recommended* means the PO hasn't applied it yet. *Not yet* means it waits for something named in the row.

Set `R=repos/vincentarisawork/velocity_download_manager_v2` before running the commands.

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
| Require a pull request before merging | On, 0 approvals | Recommended | Stops a direct push to `main` on the server, not only in the agent's guard ([CLAUDE.md](../CLAUDE.md#instructions-versus-enforcement)). With 0 approvals, the PO can still merge their own PRs; GitHub never lets an author approve their own PR |
| Require linear history | On | Recommended | Matches squash-only merging; refuses a merge commit pushed by mistake |
| Require status checks to pass (`ci-required`) | On | Not yet: CI runs by hand only (#204) | PR checks don't start on a pull request now, so a required check would stay pending and block every merge. Turn it on when the automatic triggers come back ([CI.md](CI.md)) |
| Require conversation resolution | On | Optional | Every review comment answered before merging |
| Bypass list | Empty | On | Nobody, the PO included, skips the rules by accident. The PO can still change the ruleset |
| Tag rule for `v*` | Only the PO creates, updates or deletes release tags | Not yet: comes with the release workflow (#18) | Releases are tags on verified `main` commits |
| CODEOWNERS enforcement | Off | Not yet: one maintainer | Needs a second reviewer to mean anything ([DEVELOPMENT_WORKFLOW.md](DEVELOPMENT_WORKFLOW.md)) |

Check, with the agent's account or the PO's:

```sh
gh api $R/rulesets --jq '.[] | {name, enforcement}'   # → {"name":"Main","enforcement":"active"}
gh api $R/rules/branches/main --jq '[.[].type]'       # → ["deletion","non_fast_forward"] today
```

With the recommended rules applied, the second command also lists `pull_request` and `required_linear_history`.

## Actions

| Setting | Value | State | Why | Check (*Admin*) |
|---|---|---|---|---|
| Workflow permissions | Read repository contents | On | Each job asks for any write access it needs in its own `permissions:` ([CI.md](CI.md#permissions-and-supply-chain)) | `gh api $R/actions/permissions/workflow` → `"default_workflow_permissions":"read"` |
| Allow Actions to create and approve pull requests | Off | On | No workflow approves its own changes | the same command → `"can_approve_pull_request_reviews":false` |
| Allowed actions | Allow actions created by GitHub only | Recommended (today: all actions) | The workflows use only `actions/checkout`, `actions/setup-node`, `actions/cache`, `actions/upload-artifact` and `actions/download-artifact`, plus the repository's own composite actions. Narrowing the list stops a third-party action from being added without a settings change | `gh api $R/actions/permissions` → `"allowed_actions":"selected"`, and `gh api $R/actions/permissions/selected-actions` → `"github_owned_allowed":true` |
| Require actions to be pinned to a full-length commit SHA | On | Recommended | Every action is already pinned to a SHA ([DEPENDENCIES.md](DEPENDENCIES.md)); this makes GitHub refuse one that isn't | `gh api $R/actions/permissions` → `"sha_pinning_required":true` |

Source: [managing GitHub Actions settings for a repository](https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/enabling-features-for-your-repository/managing-github-actions-settings-for-a-repository).

## Security

| Setting | Value | Plan | State | Check (*Admin*) |
|---|---|---|---|---|
| Dependabot alerts | On | Every plan | On | `gh api -i $R/vulnerability-alerts` → `HTTP/2.0 204` |
| Dependabot security updates | On (PO decision on #75) | Every plan | On | `gh api $R/automated-security-fixes` → `{"enabled":true,"paused":false}` |
| Dependabot version updates | GitHub Actions only, monthly, grouped | Every plan | On, in [`.github/dependabot.yml`](../.github/dependabot.yml) | the file itself |
| Secret scanning and push protection | On | Public repositories; private ones need GitHub Secret Protection (Team or Enterprise) | Not yet: at publication (#90) | `gh api $R --jq .security_and_analysis` |
| Code scanning and dependency review | On | Public repositories; private ones need GitHub Code Security (Team or Enterprise) | Not yet: at publication (#90) | Settings → Code security |
| Private vulnerability reporting | On | Public repositories | Not yet: at publication (#90) | `gh api $R/private-vulnerability-reporting` |

Source: [GitHub security features](https://docs.github.com/en/code-security/getting-started/github-security-features). What V2 does with an advisory is in [SECURITY.md](SECURITY.md#dependency-advisories-75).

## When something changes

- **The PO changes a setting:** update its row and the date checked, and note the issue or decision.
- **CI's automatic triggers come back:** make `ci-required` a required check in the Main ruleset.
- **The repository goes public:** apply the rows marked *at publication*, with the rest of #90's checklist.
