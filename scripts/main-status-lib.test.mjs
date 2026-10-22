import { describe, expect, it } from 'vitest';
import { TRACKING_TITLE, trackingAction } from './main-status-lib.mjs';

const RUN = { url: 'https://github.com/o/r/actions/runs/1', sha: 'abc1234def5678', ref: 'refs/heads/main' };
const green = { ok: true, rows: [{ check: 'L1 checks', job: 'linux', status: 'PASSED' }, { check: 'Windows checks', job: 'windows', status: 'SKIPPED-BY-SELECTOR' }] };
const red = {
  ok: false,
  rows: [
    { check: 'L1 checks', job: 'linux', status: 'FAIL', errors: [{ file: 'crates/velocity-core/src/lib.rs', line: 3, message: 'expected `u32`' }] },
    { check: 'Windows checks', job: 'windows', status: 'PASSED' },
    { check: 'Docs (check:links)', job: 'select', status: 'INFRA', reason: 'setup step "checkout" failed' },
  ],
};

describe('trackingAction', () => {
  it('opens a tracking issue when main turns red', () => {
    const a = trackingAction({ diagnostics: red, openIssue: null, run: RUN });
    expect(a.action).toBe('open');
    expect(a.title).toBe(TRACKING_TITLE);
    expect(a.body).toContain('abc1234');
    expect(a.body).toContain(RUN.url);
    expect(a.body).toMatch(/\| L1 checks \| linux \| FAIL \|/);
    expect(a.body).toMatch(/\| Docs \(check:links\) \| select \| INFRA \| setup step "checkout" failed \|/);
    expect(a.body).not.toMatch(/Windows checks/);
    expect(a.body).toMatch(/fix forward or revert within one working day/);
  });

  it('comments on the open issue when main is still red', () => {
    const a = trackingAction({ diagnostics: red, openIssue: 42, run: RUN });
    expect(a).toMatchObject({ action: 'comment', issue: 42 });
    expect(a.body).toMatch(/\| L1 checks \| linux \| FAIL \|/);
  });

  it('closes the open issue when main is green again', () => {
    const a = trackingAction({ diagnostics: green, openIssue: 42, run: RUN });
    expect(a).toMatchObject({ action: 'close', issue: 42 });
    expect(a.body).toMatch(/green again at abc1234/);
  });

  it('does nothing when main is green and no issue is open', () => {
    expect(trackingAction({ diagnostics: green, openIssue: null, run: RUN })).toEqual({ action: 'none' });
  });

  it('treats a missing or unreadable report as red, so a broken run is never silent', () => {
    const a = trackingAction({ diagnostics: null, openIssue: null, run: RUN });
    expect(a.action).toBe('open');
    expect(a.body).toMatch(/no ci-diagnostics report/);
  });

  it('keeps an error line short and escapes table pipes', () => {
    const long = { ok: false, rows: [{ check: 'a|b', job: 'linux', status: 'FAIL', reason: 'x'.repeat(300) }] };
    const body = trackingAction({ diagnostics: long, openIssue: null, run: RUN }).body;
    expect(body).toContain('a\\|b');
    expect(body).not.toContain('x'.repeat(201));
  });
});
