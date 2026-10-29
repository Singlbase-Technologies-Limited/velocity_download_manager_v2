## Linked issue

Closes #41.

PO decision applied: the probe never costs a second full transfer.

## Change type

- [x] Feature
- [ ] Bug fix
- [ ] Refactor
- [x] Documentation
- [ ] Build or CI
- [ ] Security
- [x] Test
- [ ] Packaging or release

## What changed

- The engine probes a link with one bounded range request before it splits the download.

**Not included:**

- Adaptive connections (#47).

## Tests

- [x] A failing test was written before the meaningful behaviour (Red first), and its failure is recorded below.
- [x] Bug fixes have a regression test, or the exception is explained.
- [x] No test was deleted, skipped or weakened, and no lint or check was relaxed, to get a pass.

Red first: `a_200_to_a_later_range_is_aborted` failed with `left: Written(4096), right: Aborted`.

Commands actually run, with their results:

```text
$ pnpm check:fast
PASS  7 of 7
```

Verification level reached (docs/TESTING.md): L3

| Check or platform | Result | Notes |
| --- | --- | --- |
| `pnpm check:fast` or `check:pr` | PASS | 7 of 7 |
| Windows | PASS | |
| Linux | PASS | The WSL clean clone |
| macOS | NOT RUN | #83 |
| CI checks (once CI exists) | NOT RUN | Paused |

## Acceptance criteria

| Criterion | Result | Evidence |
| --- | --- | --- |
| A `200` to a later range is aborted before any byte is written | PASS | `a_200_to_a_later_range_is_aborted` |

## Architecture and data

- [x] Existing code and patterns reused; any new pattern is explained (AGENTS.md, "Reuse before you add").
- [x] No architecture change without a PO decision (docs/decisions/).
- [x] Database or checkpoint changes use versioned migrations, tested from the previous schema. None in this PR.
- [x] Contract changes (commands, events, engine protocol) are versioned and tested. None in this PR.
- [x] No new dependency without PO approval and a recorded licence.

## Security and privacy

- [x] Untrusted input (URLs, names, paths, headers, responses, messages) validated at the native boundary.
- [x] Filesystem containment, link handling and deletion ownership considered.
- [x] No secret reaches logs, the database, checkpoints, temporary files, process arguments or the UI.
- [x] TLS verification and other security controls remain enabled.
- [ ] `/security-check` (and the built-in `/security-review`) run where relevant. NOT RUN: the PO runs them.

## Documentation

| Document | Updated, or why no update was needed |
| --- | --- |
| `README.md` | Reviewed; no change |
| `docs/ENGINE_INTERNALS.md` | The probe |

- [x] Commands and examples match the implementation.
- [x] New documents are in `docs/README.md`, and new terms are in `docs/GLOSSARY.md`.

## Things to check during review

- The probe's range size.

---

The reviewer checks this PR against [docs/REVIEWING.md](https://github.com/vincentarisawork/velocity_download_manager_v2/blob/main/docs/REVIEWING.md). Only the Product Owner decides the merge.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
