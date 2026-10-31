# Release process

How V2 is versioned, how a version's changelog is written, and how candidates and releases are tagged (#84). Who may tag, and the branch rules around releases, are in [DEVELOPMENT_WORKFLOW.md](DEVELOPMENT_WORKFLOW.md#releases). Building a candidate is [RELEASE_CANDIDATES.md](RELEASE_CANDIDATES.md); publishing a release is #18.

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
- **Who:** the PO, or an agent on the PO's explicit instruction for that tag.

## Before a candidate

The [`/release-candidate`](../.claude/skills/release-candidate/SKILL.md) skill walks through these, and changes nothing without the PO's approval:

1. `main` is green: the latest *Main checks* run on the commit passed.
2. `package.json` has the version to release, and the version test passes.
3. `CHANGELOG.md`'s section for it is written: `pnpm changelog <last tag or start> --heading "<version> - <date>"`, edited.
4. The PO tags `v<version>-rc.<N>` on that commit, or starts the workflow by hand ([RELEASE_CANDIDATES.md](RELEASE_CANDIDATES.md#starting-one)).
5. The candidate's `build-info.json` and `SHA256SUMS` match the commit, and the PO tests it beside V1.
