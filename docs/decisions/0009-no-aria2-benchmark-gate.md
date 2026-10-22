# ADR-0009: Build the engine first; no aria2 benchmark comparison for now

- **Status:** Accepted
- **Date:** 2026-10-22
- **Decided by:** the Product Owner
- **Issue:** #20 (closed as not planned), #21 (closed as not planned)

## Context

The research report's section 7 made a benchmark race against aria2 the gate for V2's engine. The engine prototype (#37) had to be "no worse" than the best valid aria2 configuration in every scenario before the full engine was built, and #37 depended on the harness (#20) and Windows resource measurement (#21).

Planning #20 showed what that gate costs before any engine code exists:

- **A tuning search for aria2's best settings,** per scenario. It takes hours of the PO's PC even when reduced to the settings that matter.
- **A local server with many misbehaving modes, aria2 runners, and paired-bootstrap statistics.**
- **Windows Job Object measurement** (#21).

The PO's view: V2 doesn't need to beat aria2 to be worth building. It needs an engine built the best way we know, which never corrupts a file and is really usable, measured on its own terms once it exists.

## Decision

1. **No aria2 benchmark comparison for now.** #20 (the harness against aria2) and #21 (Windows resource measurement for it) are closed as not planned. Either can be reopened if a comparison is wanted later.
2. **The engine prototype (#37) is judged on correctness,** not on a race:
   - it downloads correctly, checked by SHA-256, in both deployment modes, on Linux and on Windows;
   - it passes its section 5.4 fixture rows;
   - its report gives #38 the evidence to choose the deployment model.
3. **Correctness testing doesn't change.** Local servers that misbehave on purpose, and fixtures proving that no file is ever corrupted or wrongly marked complete, come with each engine issue (#41, #45, #46, #48 and others) and the test strategy ([TESTING.md](../TESTING.md)).
4. **V2's own benchmarks come later,** once the engine and app exist. They measure V2's speed, resource use and reliability, without an aria2 baseline, unless a later decision adds one.

## Options considered

| Option | For | Against |
|---|---|---|
| **A. Build first, benchmark V2 itself later (chosen)** | Engine and app work starts now. No hours of aria2 tuning. No aria2 reference binary to manage | No proof that V2 is at least as fast as V1 before the full engine is built. A design weakness may surface later |
| B. The report's gate: race the prototype against the best aria2 | Evidence before committing to the design | Hours to days of benchmark setup and tuning before any engine exists; a harness to build first (#20, #21) |
| C. A light comparison: a few aria2 runs, no tuning search | Some reference numbers | Weak evidence (V1's own profiles were the weak baseline the report warned about), for real cost |

## Consequences

- **Faster start:** #37 depends only on #36 and #35, both done, so the engine work can start now.
- **No speed claims:** without measurements, no speed or resource claim is made, against aria2 or otherwise, until V2's own benchmarks run (AGENTS.md, "no unmeasured performance claims").
- **Correctness carries the weight:** the guarantee against corrupted files rests on the engine's correctness tests and fixtures, which stay required. It no longer rests on a benchmark gate.
- **Documents and issues changed:**
  - **#37:** the dependency on #20 and the benchmark acceptance criterion are removed.
  - **#85:** the dependency on #20 is removed.
  - **Epic #1:** milestone 3 no longer refers to benchmark gates.
  - **Docs:** TESTING.md, CI.md, CLAUDE.md and `.claude/rules/transfer-engine.md` no longer point to #20.
- **The research report stays as it is** (`docs/research/` copies are verbatim). Its section 7 is superseded on this point by this ADR.

## Follow-up

- **#37:** the engine prototype, now unblocked.
- **#77:** stress and performance-regression checks against V2's own stored baselines. It's where V2's own benchmarks belong; its dependency on #21 is revisited when it starts.
- **Revisit** if the prototype shows a performance problem whose cause can't be found without a reference, or if a release needs a "faster than V1" claim. Then reopen #20.
