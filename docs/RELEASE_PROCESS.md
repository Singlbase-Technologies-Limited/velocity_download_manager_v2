# Release process

How V2 is versioned, how a version's changelog is written, how candidates and releases are tagged (#84), and how a release is made (#18). Who may tag, and the branch rules around releases, are in [DEVELOPMENT_WORKFLOW.md](DEVELOPMENT_WORKFLOW.md#releases). Building a candidate is [RELEASE_CANDIDATES.md](RELEASE_CANDIDATES.md).

## The version

- **[Semantic Versioning](https://semver.org/):** `MAJOR.MINOR.PATCH`. While V2 is below 1.0.0, a minor version may break things, as SemVer allows for 0.y.z. The first release candidate is `v0.1.0-rc.1` (PO decision on #84).
- **One value:** `package.json`'s `version` is the source.
  - **Tauri** reads it for the app and its packages: `src-tauri/tauri.conf.json` says `"version": "../package.json"` ([Tauri's `version`](https://v2.tauri.app/reference/config/#version)).
  - **The browser extension's manifest** is built from it (`scripts/build-extension.mjs`).
  - **Cargo** can't read another file, so `[workspace.package] version` in `Cargo.toml` repeats it, and every crate takes it (`version.workspace = true`). The programs report it through `CARGO_PKG_VERSION`: the app's About screen, the user agent and the torrent client's identity.
- **A test fails while they differ** ([`scripts/version-lib.test.mjs`](../scripts/version-lib.test.mjs)), and the release-candidate workflow refuses a tag whose version isn't `package.json`'s.
- **Changing it:** `pnpm version:set <MAJOR.MINOR.PATCH>` sets `package.json` and `Cargo.toml`, then updates only V2's own crates in `Cargo.lock` (`cargo update --workspace`). Commit the three files in one pull request with the changelog section, such as `chore: version 0.2.0 (#N)`.
- **A pre-release is a tag, not a version:** the files say `0.2.0`, and its candidates are the tags `v0.2.0-rc.1`, `v0.2.0-rc.2`.

## The changelog

[`CHANGELOG.md`](../CHANGELOG.md) has one section a version, newest first, under an `Unreleased` section for what has merged since the last one.

- **Drafted from the pull requests:** every pull request is squash-merged with a Conventional Commit title ([DEVELOPMENT_WORKFLOW.md](DEVELOPMENT_WORKFLOW.md#commits-and-pull-request-titles)), so `main`'s first-parent history is the list of changes. `pnpm changelog <from> [<to>] [--heading "<heading>"]` prints a section for the commits after `<from>` (a tag, a commit, or `start`) up to `<to>` (`origin/main` by default).
- **The groups,** in order: Breaking changes (a `!` after the type), Features (`feat`), Fixes (`fix`), Performance (`perf`), Security (`security`), Documentation (`docs`), Packaging and builds (`build`), Continuous integration (`ci`), Tests (`test`), and Other changes (`chore`, `refactor`, `style`, `revert`, and any other type). Each line is the title's description, its scope in bold, and its issue and pull request, such as `- **ui:** the column chooser keeps Name (#59, PR #200)`.
- **Titles that aren't Conventional Commits** go under Other changes as they are, and the command lists them, so they can be reworded.
- **Edited by hand into release notes:** the draft is a starting point. Before a release the PO, or an agent on the PO's request, rewrites the lines users need to read, drops what doesn't concern them, and adds upgrade notes, such as settings V1 had that V2 dropped. The rules are in [`scripts/changelog-lib.mjs`](../scripts/changelog-lib.mjs).

## Tags

- **Only on verified `main` commits:** a commit on `main` whose *Main checks* run passed.
- **`vX.Y.Z-rc.N`** for a release candidate. Pushing one starts the *Release candidate* workflow ([RELEASE_CANDIDATES.md](RELEASE_CANDIDATES.md)).
- **`vX.Y.Z`** for a release, on the commit of the candidate the PO accepted (#18).
- **Never moved or deleted.** A failed candidate is followed by `-rc.N+1`.
- **Who:** the PO only. The *Release Tags* ruleset lets only admins create, move or delete `v*` tags ([REPOSITORY_SETTINGS.md](REPOSITORY_SETTINGS.md#rules-for-the-main-branch)), so an agent prepares the exact command and the PO runs it.

## Before a candidate

The [`/release-candidate`](../.claude/skills/release-candidate/SKILL.md) skill walks through these, and changes nothing without the PO's approval:

1. `main` is green: the latest *Main checks* run on the commit passed.
2. `package.json` has the version to release, and the version test passes.
3. `CHANGELOG.md`'s section for it is written: `pnpm changelog <last tag or start> --heading "<version> - <date>"`, edited.
4. The PO tags `v<version>-rc.<N>` on that commit, or starts the workflow by hand ([RELEASE_CANDIDATES.md](RELEASE_CANDIDATES.md#starting-one)).
5. The candidate's `build-info.json` and `SHA256SUMS` match the commit, and the PO tests it beside V1.

## Making the release

Once the PO accepts a candidate, its installers are the release: the release workflow publishes them as they are, rather than build again, so what was tested is what is released (#18). Tauri's builds aren't byte-for-byte reproducible, so a rebuild couldn't match the candidate's checksums.

1. **The PO tags `vX.Y.Z`** on the accepted candidate's commit, within 14 days of its build, while its files are kept.
2. **The *Release* workflow** ([`release.yml`](../.github/workflows/release.yml)) starts on the tag:
   - **`verify`** refuses a tag that isn't `v<package.json version>`, isn't on `main`, has no green *Main checks* run, or isn't on a candidate's commit (a `v<version>-rc.N` tag; the highest N is the one released). It finds the candidate's run and artifact. Its files gone, it refuses: tag a new candidate.
   - **`assemble`** downloads the candidate, checks every file against its `SHA256SUMS`, and names them `velocity-v2-<version>-<what>`, such as `velocity-v2-0.1.0-windows-x64-setup.exe`, with a new `SHA256SUMS`. It attests their build provenance with GitHub's [artifact attestations](https://docs.github.com/en/actions/security-for-github-actions/using-artifact-attestations), and takes the notes from `CHANGELOG.md`'s `## <version>` section; without one it refuses.
   - **`draft`** waits in the `release` environment for the PO's approval ([below](#the-release-environment)), then creates a **draft** GitHub release with the files and the notes. It is the only job that may write.
3. **The PO publishes the draft,** on GitHub, after a last look. Nothing publishes it automatically.

### The release environment

The PO creates it once, in Settings, Environments: named `release`, with the PO as its **required reviewer** (self-review allowed, since the PO approves their own release), and deployments limited to tags `v*`. On GitHub Free, Pro and Team plans, required reviewers are only available for public repositories ([deployments and environments](https://docs.github.com/en/actions/reference/workflows-and-actions/deployments-and-environments)), so it is set up once the repository is public. Without it, the `draft` job would run without waiting.

Anyone can check a downloaded file's provenance: `gh attestation verify <file> --repo Singlbase-Technologies-Limited/velocity_download_manager_v2`. The rules are in [`scripts/release-lib.mjs`](../scripts/release-lib.mjs).
