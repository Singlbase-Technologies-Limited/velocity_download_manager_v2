<!--
Title: a Conventional Commit, such as `feat(engine): add bounded range probe (#41)`.
PRs are squash-merged. A PR with one commit keeps that commit's message on `main`,
so the commit message must follow the convention too (docs/DEVELOPMENT_WORKFLOW.md).
-->

## Linked issue

<!--
Write "Closes #N" only if merging completes every acceptance criterion of #N; otherwise "Part of #N".
GitHub treats "close", "fix" and "resolve" followed by #N as closing keywords anywhere in this text,
even in a sentence like "doesn't close #N". Don't write them for other issues.
-->

Part of #

## Change type

- [ ] Feature
- [ ] Bug fix
- [ ] Refactor
- [ ] Documentation
- [ ] Build or CI
- [ ] Security
- [ ] Test
- [ ] Packaging or release

## What changed

<!-- The user-visible or technical change, and the important choices. Include PO decisions applied. -->

-

**Not included:**

-

## Tests

- [ ] A failing test was written before the meaningful behaviour (Red first), and its failure is recorded below.
- [ ] Bug fixes have a regression test, or the exception is explained.
- [ ] No test was deleted, skipped or weakened, and no lint or check was relaxed, to get a pass.

Commands actually run, with their results:

```text
$ pnpm check:pr
…
```

Verification level reached (docs/TESTING.md): L1 / L2 / L3 / L4

| Check or platform | Result | Notes |
| --- | --- | --- |
| `pnpm check:fast` or `check:pr` | PASS / FAIL / BLOCKED / NOT RUN | |
| Windows | PASS / FAIL / BLOCKED / NOT RUN | |
| Linux | PASS / FAIL / BLOCKED / NOT RUN | |
| macOS | NOT RUN unless a Mac was used | |
| CI checks (once CI exists) | Only results you have seen; a job not selected is NOT RUN | |

<!-- Never report a check as PASS without running it. Explain every NOT RUN and BLOCKED. -->

## Acceptance criteria

| Criterion | Result | Evidence |
| --- | --- | --- |
| | | |

## Architecture and data

- [ ] Existing code and patterns reused; any new pattern is explained (AGENTS.md, "Reuse before you add").
- [ ] No architecture change without a PO decision (docs/decisions/).
- [ ] Database or checkpoint changes use versioned migrations, tested from the previous schema.
- [ ] Contract changes (commands, events, engine protocol) are versioned and tested.
- [ ] No new dependency without PO approval and a recorded licence.

## Security and privacy

- [ ] Untrusted input (URLs, names, paths, headers, responses, messages) validated at the native boundary.
- [ ] Filesystem containment, link handling and deletion ownership considered.
- [ ] No secret reaches logs, the database, checkpoints, temporary files, process arguments or the UI.
- [ ] TLS verification and other security controls remain enabled.
- [ ] `/security-check` (and the built-in `/security-review`) run where relevant.

## Documentation

| Document | Updated, or why no update was needed |
| --- | --- |
| `README.md` | |
| | |

- [ ] Commands and examples match the implementation.
- [ ] New documents are in `docs/README.md`, and new terms are in `docs/GLOSSARY.md`.

## Things to check during review

<!-- Anything the reviewer should look at closely, open questions, risks and known limitations. -->

-

---

The reviewer checks this PR against [docs/REVIEWING.md](https://github.com/Singlbase-Technologies-Limited/velocity_download_manager_v2/blob/main/docs/REVIEWING.md). Only the Product Owner decides the merge.
