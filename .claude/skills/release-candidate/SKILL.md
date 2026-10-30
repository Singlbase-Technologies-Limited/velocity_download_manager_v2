---
name: release-candidate
description: Prepare a V2 release candidate. Checks the main commit and its Main checks run, the version in package.json and Cargo.toml, and the changelog section, then reports what the PO needs to tag or start. It never tags, pushes, starts a workflow or publishes anything without the PO's explicit approval of that step. Manual.
disable-model-invocation: true
argument-hint: "<version, such as 0.1.0> [rc number]"
---

# Prepare release candidate $ARGUMENTS

The procedure is [docs/RELEASE_PROCESS.md](../../../docs/RELEASE_PROCESS.md), and the build [docs/RELEASE_CANDIDATES.md](../../../docs/RELEASE_CANDIDATES.md). Report each check below as PASS, FAIL, BLOCKED or NOT RUN, with the command and its real output. Change nothing until the PO approves the step.

## 1. The commit

- **Which:** `git fetch origin main`, then `git rev-parse origin/main`. That is the candidate's commit unless the PO names an earlier one on `main` (`git merge-base --is-ancestor <commit> origin/main`).
- **Green:** its *Main checks* run on `main` passed: `GH_TOKEN="$(gh auth token -u velocity-claude-agent)" gh run list --workflow main.yml --branch main --commit <commit> --json conclusion,url`. Without a green run, stop: the workflow would refuse it. Starting one needs the PO's go-ahead (docs/CI.md, "While CI runs by hand only").

## 2. The version

- `package.json`'s `version` is the one asked for, and `Cargo.toml`'s `[workspace.package] version` equals it. The test is `scripts/version-lib.test.mjs`.
- If it isn't, propose `pnpm version:set <version>` in its own pull request, with the changelog section below. Don't commit it to `main`.
- **The tag** will be `v<version>-rc.<N>`: the next N after the existing tags (`git tag --list 'v<version>-rc.*'`). Tags are never moved.

## 3. The changelog

- `pnpm changelog <last tag, or start> --heading "<version> - <today>"` drafts the section. List the subjects it reports as not Conventional Commits.
- `CHANGELOG.md` on that commit has the section, edited into notes users can read. If not, propose the edit in the version's pull request.

## 4. What to ask the PO

Summarise the commit, its Main checks run, the version, the tag, and the changelog section. Then ask, as a multiple-choice question, which step to take:

- the PO pushes the tag `v<version>-rc.<N>` themselves;
- the PO approves that the agent pushes that one tag (`git tag v<version>-rc.<N> <commit>` and `git push origin v<version>-rc.<N>`, as the agent account);
- the PO approves a hand-started build without a tag: `gh workflow run release-candidate.yml --ref main -f commit=<commit>`.

## 5. After the build

- Wait for the run with a loop on `gh run view <id> --json status,conclusion`; `gh run watch` has exited early before.
- Download the candidate artifact, run `sha256sum --check --strict SHA256SUMS`, and check that `build-info.json`'s `commit` and `version` are the candidate's.
- Report the artifact's name and link, and remind the PO of the test beside V1 and the backup before installing (docs/RELEASE_CANDIDATES.md).
