# Research

This folder keeps the research that V2's decisions were based on, so a decision can always be traced to its evidence. Research documents are **inputs, not decisions**. When a decision overrides a research recommendation, the decision record says so. Decision records live in [docs/decisions/](../decisions/README.md).

| Date | Document | Status | Notes |
|---|---|---|---|
| 2026-10-21 | [Velocity libcurl download engine — research report, revision 2](2026-10-21-libcurl-engine-research-r2.md) | Research baseline for V2 | Written about V1 at `e9d717a8`. Sections 4 (architecture) and 12 (recommendation) shaped [ADR-0001](../decisions/0001-v2-direction.md). Its aria2 assumptions (legacy engine, fallback, migration of aria2 downloads) do not apply to V2. |
| 2026-10-22 | [Engine prototype results](engine-prototype-results.md) | Evidence for #38 | V2's own findings from the #37 prototype: correctness in both modes on Windows and Linux. Performance not measured. |
| 2026-10-22 | [Engine mode costs](engine-mode-costs.md) | Evidence for #38 | What the child process costs against in-process: throughput, CPU, memory, latency, startup, shutdown and crash detection, on Windows and Linux (loopback). |

Copies of outside research here are verbatim exports. Corrections go into a new revision with its own date, never into an old copy.
