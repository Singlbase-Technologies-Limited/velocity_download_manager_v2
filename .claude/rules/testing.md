---
# pending: "src-tauri/tests/**/*" (#58)
paths:
  - "**/*.test.{ts,tsx,mjs}"
  - "crates/*/tests/**/*"
  - "src-tauri/tests/**/*"
---

# Tests

The levels, commands, test strategy and fixture policy are in [docs/TESTING.md](../../docs/TESTING.md).

- **Red first:** for meaningful behaviour, the new test runs and fails for the right reason before the code exists. The PR records that failure.
- **Behaviour, not internals:** test behaviour and external contracts. A refactor that keeps the behaviour should not break a test.
- **Deterministic and bounded:** every test is deterministic, isolated and bounded by timeouts. Use local servers and fixtures, never the public internet. Seed random generators and print the seed when a test fails.
- **Fakes and real processes:** fakes are fine at unit boundaries. Claims about the engine, processes, the file system or crash recovery need real processes and real files.
- **Never weaken a test to get a pass:** don't delete, skip, loosen or rewrite it. A changed assertion is explained in the PR.
- **Platform results are honest:** a check is PASS, FAIL, BLOCKED or NOT RUN, per platform. A Linux pass says nothing about Windows.
- **Iterate, then check:** run `pnpm test:focused -- <area>` while iterating, and the level the change needs (`pnpm check:pr`) before review.
