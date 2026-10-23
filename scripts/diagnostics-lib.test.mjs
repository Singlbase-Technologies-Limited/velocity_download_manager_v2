import { describe, expect, it } from 'vitest';
import { buildDiagnostics, diagnosticsMarkdown, extractErrors, latestReports } from './diagnostics-lib.mjs';

// Trimmed from real tool output captured on 2026-10-21 (Windows paths and colours included).
const ROOT = 'C:\\work\\v2';
const FMT = `Diff in \\\\?\\C:\\work\\v2\\src-tauri\\src\\lib.rs:54:
 }
\u001b[31m-fn lint_bait(v: &Vec<i32>) -> usize { v.len() }
\u001b[0m\u001b[32m+fn lint_bait(v: &Vec<i32>) -> usize {
Diff in /home/runner/work/v2/v2/crates/velocity-core/src/lib.rs:3:
-a
+b
`;
const CLIPPY = `    Checking velocity-core v0.1.0 (C:\\work\\v2\\crates\\velocity-core)
error: writing \`&Vec\` instead of \`&[_]\` involves a new object where a slice will do
  --> src-tauri\\src\\lib.rs:57:17
   |
57 | fn lint_bait(v: &Vec<i32>) -> usize { v.len() }
   = note: \`-D clippy::ptr-arg\` implied by \`-D warnings\`

error[E0308]: mismatched types
  --> crates/velocity-core/src/lib.rs:12:5
   |

error: could not compile \`velocity-desktop-v2\` (lib test) due to 2 previous errors
warning: build failed, waiting for other jobs to finish...
`;
const TEST = `     Running unittests src\\lib.rs (C:\\work\\v2\\target\\debug\\deps\\velocity_core-0bcf.exe)
---- planted_failures::core_planted_failure stdout ----

thread 'planted_failures::core_planted_failure' (31580) panicked at crates\\velocity-core\\src\\lib.rs:70:9:
assertion \`left == right\` failed: planted failure in velocity-core
note: run with \`RUST_BACKTRACE=1\` environment variable to display a backtrace

error: test failed, to rerun pass \`-p velocity-core --lib\`
     Running unittests src\\lib.rs (C:\\work\\v2\\target\\debug\\deps\\velocity_desktop_lib-3e53.exe)
---- planted_failures::desktop_planted_failure stdout ----

thread 'planted_failures::desktop_planted_failure' (31584) panicked at src-tauri\\src\\lib.rs:63:9:
planted failure in velocity-desktop-v2
note: run with \`RUST_BACKTRACE=1\` environment variable to display a backtrace

error: 2 targets failed:
`;
const TSC = `src/planted.ts(1,14): error TS2322: Type 'string' is not assignable to type 'number'.
`;
const ESLINT = `
C:\\work\\v2\\src\\App.tsx
  12:7  error  'unused' is assigned a value but never used  @typescript-eslint/no-unused-vars
  20:1  error  Unexpected any. Specify a different type      @typescript-eslint/no-explicit-any

✖ 2 problems (2 errors, 0 warnings)
`;
const VITEST = ` ❯ src/planted.test.ts (3 tests | 3 failed) 24ms
⎯⎯⎯⎯⎯⎯⎯ Failed Tests 3 ⎯⎯⎯⎯⎯⎯⎯

 FAIL  src/planted.test.ts > planted > fails one
AssertionError: expected 1 to be 2 // Object.is equality

 ❯ src/planted.test.ts:3:37
      1| import { describe, expect, it } from 'vitest';

 FAIL  src/planted.test.ts > planted > fails two
AssertionError: expected 'a' to be 'b' // Object.is equality

 ❯ src/planted.test.ts:4:39

 FAIL  src/planted.test.ts > planted > fails three
AssertionError: expected [ 1 ] to deeply equal [ 2 ]

 ❯ src/planted.test.ts:5:41
`;
const LINKS = `BROKEN  docs/A.md: B.md (missing file)
BROKEN  docs/A.md: #nope (missing anchor)
check:links: 37 Markdown file(s), 2 broken link(s).
`;

describe('extractErrors', () => {
  it('rustfmt: one entry per file with a diff, paths relative to the repository', () => {
    expect(extractErrors('rust-fmt', FMT, { root: ROOT })).toEqual([
      { file: 'src-tauri/src/lib.rs', line: 54, message: 'Formatting differs from rustfmt' },
      { file: 'crates/velocity-core/src/lib.rs', line: 3, message: 'Formatting differs from rustfmt' },
    ]);
  });

  it('clippy and compiler errors: message with file:line:col, without the summary lines', () => {
    expect(extractErrors('rust-clippy', CLIPPY, { root: ROOT })).toEqual([
      { file: 'src-tauri/src/lib.rs', line: 57, col: 17, message: 'writing `&Vec` instead of `&[_]` involves a new object where a slice will do' },
      { file: 'crates/velocity-core/src/lib.rs', line: 12, col: 5, message: 'error[E0308]: mismatched types' },
    ]);
  });

  it('cargo test: every failing test across binaries, with its panic location and message', () => {
    expect(extractErrors('rust-test', TEST, { root: ROOT })).toEqual([
      {
        file: 'crates/velocity-core/src/lib.rs',
        line: 70,
        col: 9,
        test: 'planted_failures::core_planted_failure',
        message: 'assertion `left == right` failed: planted failure in velocity-core',
      },
      {
        file: 'src-tauri/src/lib.rs',
        line: 63,
        col: 9,
        test: 'planted_failures::desktop_planted_failure',
        message: 'planted failure in velocity-desktop-v2',
      },
    ]);
  });

  it('cargo test: compile errors in test code are reported too', () => {
    expect(extractErrors('rust-test', CLIPPY, { root: ROOT })).toHaveLength(2);
  });

  it('tsc', () => {
    expect(extractErrors('tsc', TSC, { root: ROOT })).toEqual([
      { file: 'src/planted.ts', line: 1, col: 14, message: "TS2322: Type 'string' is not assignable to type 'number'." },
    ]);
  });

  it('ESLint stylish output', () => {
    expect(extractErrors('eslint', ESLINT, { root: ROOT })).toEqual([
      { file: 'src/App.tsx', line: 12, col: 7, message: "'unused' is assigned a value but never used (@typescript-eslint/no-unused-vars)" },
      { file: 'src/App.tsx', line: 20, col: 1, message: 'Unexpected any. Specify a different type (@typescript-eslint/no-explicit-any)' },
    ]);
  });

  it('Vitest: each failed test with its location and assertion', () => {
    const errors = extractErrors('vitest', VITEST, { root: ROOT });
    expect(errors).toHaveLength(3);
    expect(errors[0]).toEqual({
      file: 'src/planted.test.ts',
      line: 3,
      col: 37,
      test: 'planted > fails one',
      message: 'AssertionError: expected 1 to be 2 // Object.is equality',
    });
  });

  it('check:links', () => {
    expect(extractErrors('links', LINKS, { root: ROOT })).toEqual([
      { file: 'docs/A.md', message: 'B.md (missing file)' },
      { file: 'docs/A.md', message: '#nope (missing anchor)' },
    ]);
  });

  it('caps the number of errors per check and says how many were left out', () => {
    const many = Array.from({ length: 30 }, (_, i) => `src/f.ts(${i + 1},1): error TS1: x`).join('\n');
    const errors = extractErrors('tsc', many, { root: ROOT, max: 5 });
    expect(errors).toHaveLength(6);
    expect(errors[5]).toEqual({ message: '25 more not shown; see the job log' });
  });

  it('falls back to the last output lines when it recognises no error', () => {
    expect(extractErrors('tsc', 'something odd\nexploded\n', { root: ROOT })).toEqual([{ message: 'something odd\nexploded' }]);
  });
});

// A complete, passing PR run: `select` (with the docs check) and `code` both ran.
const passing = () => ({
  needs: { select: { result: 'success', outputs: { code: 'true', docs: 'true' } }, code: { result: 'success' } },
  reports: {
    select: { job: 'select', infraFailure: null, checks: [{ id: 'links', name: 'Docs (check:links)', status: 'pass', errors: [] }] },
    code: { job: 'code', infraFailure: null, checks: [{ id: 'rust-test', name: 'Rust unit tests', status: 'pass', errors: [] }] },
  },
  jobs: [
    { id: 'select', mandatory: true },
    { id: 'code', mandatory: true, selectedBy: 'select', selectedOutput: 'code' },
  ],
});
const statusOf = (d, check) => d.rows.find((r) => r.check === check)?.status;

describe('buildDiagnostics', () => {
  it('passes a run where every check passed', () => {
    const d = buildDiagnostics(passing());
    expect(d.ok).toBe(true);
    expect(d.rows.map((r) => r.status)).toEqual(['PASSED', 'PASSED']);
  });

  it('reports every failed check in its own row and fails (scenario A)', () => {
    const input = passing();
    input.needs.code.result = 'failure';
    input.needs.select.result = 'failure';
    input.reports.select.checks[0] = { id: 'links', name: 'Docs (check:links)', status: 'fail', errors: [{ file: 'docs/A.md', message: 'B.md (missing file)' }] };
    input.reports.code.checks = ['rust-fmt', 'rust-clippy', 'rust-test', 'tsc'].map((id) => ({
      id,
      name: id,
      status: id === 'tsc' ? 'pass' : 'fail',
      errors: id === 'tsc' ? [] : [{ message: `${id} broke` }],
    }));
    const d = buildDiagnostics(input);
    expect(d.ok).toBe(false);
    expect(d.rows.filter((r) => r.status === 'FAILED').map((r) => r.check)).toEqual([
      'Docs (check:links)',
      'rust-fmt',
      'rust-clippy',
      'rust-test',
    ]);
    expect(statusOf(d, 'tsc')).toBe('PASSED');
  });

  it('treats a job skipped by the selector as passing, with the reason (not mandatory to run)', () => {
    const input = passing();
    input.needs.select.outputs.code = 'false';
    input.needs.code.result = 'skipped';
    delete input.reports.code;
    const d = buildDiagnostics(input);
    expect(d.ok).toBe(true);
    expect(d.rows.find((r) => r.job === 'code')).toMatchObject({ status: 'SKIPPED-BY-SELECTOR' });
  });

  it('fails a mandatory job skipped without a selector decision, naming the prerequisite (BLOCKED)', () => {
    const input = passing();
    input.needs.select = { result: 'failure', outputs: {} };
    input.reports.select = { job: 'select', infraFailure: null, checks: [{ id: 'select', name: 'Job selection', status: 'fail', errors: [{ message: 'git merge-base failed' }] }] };
    input.needs.code.result = 'skipped';
    delete input.reports.code;
    const d = buildDiagnostics(input);
    expect(d.ok).toBe(false);
    expect(d.rows.find((r) => r.job === 'code')).toMatchObject({ status: 'BLOCKED', reason: expect.stringContaining('select') });
  });

  it('classifies a failed setup step as INFRA, not as a code defect (scenario D)', () => {
    const input = passing();
    input.needs.code.result = 'failure';
    input.reports.code = { job: 'code', infraFailure: 'setup', checks: [] };
    const d = buildDiagnostics(input);
    expect(d.ok).toBe(false);
    expect(d.rows.find((r) => r.job === 'code')).toMatchObject({ status: 'INFRA', reason: expect.stringContaining('setup') });
    expect(d.rows.some((r) => r.status === 'FAILED')).toBe(false);
  });

  it('classifies a failed job without any report as INFRA (it died before reporting)', () => {
    const input = passing();
    input.needs.code.result = 'failure';
    delete input.reports.code;
    expect(buildDiagnostics(input).rows.find((r) => r.job === 'code')?.status).toBe('INFRA');
  });

  it('reports a cancelled mandatory job as CANCELLED and fails', () => {
    const input = passing();
    input.needs.code.result = 'cancelled';
    delete input.reports.code;
    const d = buildDiagnostics(input);
    expect(d.ok).toBe(false);
    expect(d.rows.find((r) => r.job === 'code')?.status).toBe('CANCELLED');
  });

  // Scenario F: never green with a failed, cancelled or unexplained-skipped mandatory job.
  it.each([
    ['failure', 'true'],
    ['cancelled', 'true'],
    ['skipped', 'true'],
    ['skipped', undefined],
  ])('never passes when code is %s (selected: %s)', (result, selected) => {
    const input = passing();
    input.needs.code.result = result;
    input.needs.select.outputs.code = selected;
    delete input.reports.code;
    expect(buildDiagnostics(input).ok).toBe(false);
  });

  it('never passes when a job succeeded but its report lists a failed check', () => {
    const input = passing();
    input.reports.code.checks[0].status = 'fail';
    expect(buildDiagnostics(input).ok).toBe(false);
  });
});

describe('diagnosticsMarkdown', () => {
  it('lists every row and the first errors with file:line', () => {
    const input = passing();
    input.needs.code.result = 'failure';
    input.reports.code.checks[0] = {
      id: 'rust-test',
      name: 'Rust unit tests',
      status: 'fail',
      errors: [{ file: 'src-tauri/src/lib.rs', line: 63, col: 9, test: 't', message: 'boom' }],
    };
    const md = diagnosticsMarkdown(buildDiagnostics(input));
    expect(md).toMatch(/\| Rust unit tests \| code \| FAILED \|/);
    expect(md).toContain('`src-tauri/src/lib.rs:63:9`');
    expect(md).toMatch(/Result: \*\*failed\*\*/);
  });
});

describe('latestReports', () => {
  const report = (job, attempt, status) => ({
    job,
    ...(attempt === undefined ? {} : { attempt }),
    infraFailure: null,
    checks: [{ id: 'tests', name: 'Tests', status, errors: [] }],
  });

  it('keeps each job’s report from its newest attempt, so a re-run that passed counts', () => {
    const latest = latestReports([report('code', 2, 'pass'), report('code', 1, 'fail'), report('select', 1, 'pass')]);
    expect(latest.code.attempt).toBe(2);
    expect(latest.code.checks[0].status).toBe('pass');
    expect(latest.select.attempt).toBe(1);
  });

  it('reads a report without an attempt as the first', () => {
    const latest = latestReports([report('code', undefined, 'fail'), report('code', 2, 'pass')]);
    expect(latest.code.checks[0].status).toBe('pass');
  });
});
