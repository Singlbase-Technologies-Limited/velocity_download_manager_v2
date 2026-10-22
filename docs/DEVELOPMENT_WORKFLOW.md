# Development workflow

V2 follows a main-centred workflow: every change goes from a GitHub issue, to a short-lived branch, to a pull request into `main`, to a squash merge by the Product Owner. There are no permanent `develop` or `staging` branches. Release candidates and releases are tags on verified `main` commits.

Each rule below is marked as **GitHub-documented** (with a link to the source) or **project judgment** (decided for V2; change it only through the Product Owner).

## Principles

- **`main` is the only long-lived branch, and it is always releasable.** *Project judgment* (PO decision, 2026-10-21). The model is [GitHub flow](https://docs.github.com/en/get-started/using-github/github-flow): branch from the default branch, open a pull request, review it, merge it, delete the branch (*GitHub-documented*).
- **Every change starts from an issue.** *Project judgment.* No issue, no branch.
- **Work follows the roadmap.** *Project judgment* (PO decision, 2026-10-22). Epic #1 lists every issue by phase (A to H), and the milestones R1 to R10 set the order of work. The next issue comes from the earliest milestone that still has open, unblocked work. An agent proposes it and the PO confirms it, or picks another.
- **The Product Owner decides merges, releases and repository settings.** *Project judgment.* Agents and contributors prepare work. They never merge their own pull requests, tag releases or change settings without the PO's explicit go-ahead.

## Branches

- **Name:** `<type>/<issue>-<slug>`, created from the latest `main`. *Project judgment.* Examples: `feat/41-range-probe`, `fix/2-product-name-and-linux-check`, `docs/6-main-centred-workflow`.
- **Types:** `feat`, `fix`, `chore`, `docs`, `test`, `build`, `ci`, `security`, `perf`, `refactor`. They match the Conventional Commit types below. *Project judgment.*
- **Scope:** one issue per branch, and one agent or person per branch. *Project judgment.* If several agents ever work in parallel, each uses its own [git worktree](https://git-scm.com/docs/git-worktree), so no two share a working directory.
- **Short-lived:** a branch exists only until its PR is merged or closed. *Project judgment.* GitHub deletes merged head branches automatically; the setting is on ([automatic deletion](https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/configuring-pull-request-merges/managing-the-automatic-deletion-of-branches), *GitHub-documented*).
- **Stale branches:** branches with no activity for 14 days are listed in a weekly review and either resumed or closed. Branches of PRs closed without merging are deleted. *Project judgment.*

## Commits and pull-request titles

- **Format:** [Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/), `<type>[optional scope]: <description>` (*documented in the specification*). Examples: `feat(engine): add bounded range probe`, `fix: keep line endings stable on Windows`.
- **The PR title and the commit on `main` must both follow the convention.** PRs are squash-merged. With the repository's current setting (`squash_merge_commit_title = COMMIT_OR_PR_TITLE`), a PR with several commits gets the PR title as its commit message on `main`, and a PR with a single commit keeps that commit's message ([squash merge commit messages](https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/configuring-pull-request-merges/configuring-commit-squashing-for-pull-requests), *GitHub-documented*). Whoever merges checks the proposed message before confirming. *Project judgment.* Commits on the branch may be less formal, but each should still build.
- **Breaking changes:** mark them with `!` after the type, or with a `BREAKING CHANGE:` footer (*documented in the specification*).
- **Agent commits** end with a `Co-Authored-By:` trailer naming the model. *Project judgment.*
- **Don't write `close #N`, `fixes #N` or `resolves #N` in a PR description or commit unless merging should close that issue.** GitHub treats these phrases as closing keywords, even inside a sentence like "doesn't close #2" ([linking a PR to an issue](https://docs.github.com/en/issues/tracking-your-work-with-issues/using-issues/linking-a-pull-request-to-an-issue), *GitHub-documented*). Use "Part of #N" or "Refs #N" for partial work.

## Pull requests

- **Base:** always `main`. *Project judgment.*
- **Size:** one focused issue per PR. Split it when a review would take more than about one sitting. Open a **draft** PR for work in progress. *Project judgment.*
- **Content:** the linked issue, what changed, the tests written first, the commands actually run with their results, and platform coverage marked PASS, FAIL, BLOCKED or NOT RUN. The PR template (#5) asks for these, and [REVIEWING.md](REVIEWING.md) is what the reviewer checks. *Project judgment.*
- **Checks before review:** `pnpm check:fast` (L1) locally. Once CI exists (#12–#14, #91), the CI checks must pass too. *Project judgment.*
- **Keeping up with `main`:** before the merge, bring the branch up to date by merging `main` into it (`git fetch origin && git merge origin/main`), then rerun the checks. Don't rebase a branch that has already been pushed, because that would need a force push. *Project judgment.* An unpushed branch may be rebased onto `main` instead.
- **Authorship:** Claude Code opens PRs as the machine account **@velocity-claude-agent**, so the PO can review and approve them. GitHub does not let authors approve their own pull requests ([required reviews](https://docs.github.com/en/pull-requests/collaborating-with-pull-requests/reviewing-changes-in-pull-requests/approving-a-pull-request-with-required-reviews), *GitHub-documented*). Decision recorded in #89.

## Merging

- **Squash merge only.** Each PR becomes one commit on `main`. Squash merges are fast-forward, so `main` keeps a linear history ([merge methods](https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/configuring-pull-request-merges/about-merge-methods-on-github), *GitHub-documented*). Merge commits and rebase merging are disabled in the repository settings.
- **Only the Product Owner decides merges.** The PO merges, or approves one specific PR for the agent to merge, for example with "I approve the PR". The agent then squash-merges that PR alone with `gh pr merge <N> --squash --delete-branch`. Approval of one PR never carries over to another. *Project judgment* (PO decision, 2026-10-21).
- **No server-side protection yet.** This private repository is on GitHub Free, where rulesets and protected branches are unavailable (the API returned HTTP 403 on 2026-10-21; [plans](https://docs.github.com/en/get-started/learning-about-github/githubs-plans)). The PO deferred protection (#89). Until then, nobody pushes to `main` directly. For the agent, deny rules in `.claude/settings.json` block `git push` to `main`, force pushes and `git reset --hard`, and merges, releases and tag pushes prompt the PO (#4). These rules match command text only. The agent guard (`scripts/agent-guard.mjs`, #98), a `PreToolUse` hook on every Bash and PowerShell command, blocks the same three actions in any spelling, including `git -C . push origin main`, `bash -c '…'` wrappers and git aliases, and also in bypass-permissions mode. Both are a safety net, not a security boundary.
- **After the merge:** update local `main` (`git switch main && git pull --ff-only`), check that the issue is closed (or still open, for "Part of #N" work) and that its board item shows the right status, then tick the issue's box in epic #1. GitHub updates the epic's sub-issue progress bar by itself, but never ticks the checkboxes in its description. *Project judgment* (PO decision, 2026-10-22).
- **If `main` breaks:** fix forward with a `fix/` PR, or revert the offending squash commit through a PR, within one working day. *Project judgment.* The red run opens the issue "main is failing", and the next green run closes it ([CI.md](CI.md#when-main-breaks)).

## Releases

- **Versions:** [Semantic Versioning](https://semver.org/) tags `vX.Y.Z` (*documented in the specification*). While V2 is below 1.0.0, minor versions may contain breaking changes, as SemVer allows for 0.y.z.
- **Release candidates:** tags `vX.Y.Z-rc.N` on a `main` commit whose checks passed. CI builds downloadable installers from them (#17). *Project judgment.*
- **Releases:** a tag `vX.Y.Z` on the commit of an accepted release candidate. The release workflow stops at a PO approval gate and creates only a draft release (#18). *Project judgment.*
- **Tags are never moved or deleted** once pushed. If a candidate fails, the next one is `-rc.N+1`. *Project judgment.*
- **Who tags:** the PO, or an agent on the PO's explicit instruction for that specific tag. *Project judgment.*
- The changelog and version synchronisation are defined in #84.

## Exceptions, each with a lifecycle

- **Hotfix:** an ordinary `fix/<issue>-<slug>` branch into `main`, followed by a patch tag. There is no hotfix branch type. *Project judgment.*
- **Patching an older release:** a `release/vX.Y` branch, created only when an already-released version needs a fix and `main` has moved on to incompatible work. *Project judgment.*
  - The fix lands on `main` first and is cherry-picked onto `release/vX.Y`.
  - The branch is deleted when that version is no longer supported.
- **Anything else long-lived** needs a PO-approved decision record (ADR, #8). *Project judgment.*

## Worked examples

### 1. Feature PR

```sh
git switch main && git pull --ff-only
git switch -c feat/41-range-probe
# write a failing test, make it pass, refactor (TDD)
pnpm check:fast
git push -u origin feat/41-range-probe
gh pr create --base main --title "feat(engine): add bounded range probe" --body "Part of #41 …"
```

The PO reviews and squash-merges it, or approves it for the agent to merge. The commit on `main` is `feat(engine): add bounded range probe (#<PR>)`, and GitHub deletes the branch. Locally, run `git switch main && git pull --ff-only && git branch -d feat/41-range-probe`.

### 2. Fix and patch release

`v0.4.0` is released, and a bug report arrives as issue #120.

```sh
git switch main && git pull --ff-only
git switch -c fix/120-retry-after-parsing
# write a regression test that fails, fix, refactor
gh pr create --base main --title "fix(engine): honour Retry-After dates" --body "Fixes #120 …"
```

Here the closing keyword is intended. After the PO merges, a release candidate `v0.4.1-rc.1` is tagged on that `main` commit (example 3), then released as `v0.4.1` (example 4).

### 3. Release candidate

```sh
git switch main && git pull --ff-only
git log -1 --format=%H          # the commit the PO chose; its checks on main passed
git tag -a v0.5.0-rc.1 -m "Release candidate 0.5.0-rc.1"
git push origin v0.5.0-rc.1      # CI builds downloadable installers (#17)
```

The PO installs the release candidate. If it fails, the fixes go through normal PRs and the next candidate is `v0.5.0-rc.2`.

### 4. Release

```sh
git tag -a v0.5.0 <commit of the accepted rc> -m "Velocity Download Manager 0.5.0"
git push origin v0.5.0           # release workflow stops at the PO approval gate (#18)
```

### 5. Patching an old release via `release/vX.Y`

`main` is at 0.7 with incompatible changes, and 0.5 users need a security fix (issue #150).

```sh
# 1. fix on main first (normal fix PR, merged by the PO), commit abc1234 on main
# 2. create the release branch from the last 0.5 tag (once)
git switch -c release/v0.5 v0.5.0 && git push -u origin release/v0.5
# 3. cherry-pick through a PR into release/v0.5
git switch -c fix/150-backport-v0.5 release/v0.5
git cherry-pick -x abc1234
gh pr create --base release/v0.5 --title "fix: backport #150 to 0.5" --body "Refs #150 …"
# 4. after the PO merges: tag v0.5.1 on release/v0.5
# 5. when 0.5 is no longer supported: delete release/v0.5 (the tags remain)
```

## When the project grows

These are for later, and will be revisited when the situation arises. *Project judgment.*

- **External contributors:** pull requests from forks, and approval required before first-time contributors' workflows run. Open-source readiness is #90.
- **More maintainers:** a `CODEOWNERS` file per area and a second reviewer. This needs GitHub Pro or Team, or a public repository, to be enforced (#7).
- **Many parallel PRs:** consider a merge queue only if `main` breaks often because PRs pass separately but fail together.
