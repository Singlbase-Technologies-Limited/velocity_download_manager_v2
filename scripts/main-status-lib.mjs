// Pure logic for the broken-main tracking issue (#16, docs/CI.md "When main breaks"): what a
// finished `main` run does with the issue titled TRACKING_TITLE. A red run opens it, or comments
// on it when it's already open; the next green run closes it. scripts/ci-main-status.mjs runs it.

export const TRACKING_TITLE = 'main is failing';
/** Statuses that count as passing (scripts/diagnostics-lib.mjs, buildDiagnostics). */
const PASSING = new Set(['PASSED', 'SKIPPED-BY-SELECTOR']);
const MAX_CELL = 200;

const cell = (s) => {
  const text = String(s ?? '').replace(/\s+/g, ' ').replaceAll('|', '\\|');
  return text.length > MAX_CELL ? `${text.slice(0, MAX_CELL - 3)}...` : text;
};

function firstError(row) {
  const e = row.errors?.[0];
  if (!e) return row.reason ?? '';
  const where = e.file ? `${e.file}${e.line ? `:${e.line}` : ''}: ` : '';
  return `${where}${e.message ?? ''}`;
}

function failureBody(diagnostics, run) {
  const sha = run.sha.slice(0, 7);
  const head = `\`main\` failed at ${sha} ([run](${run.url})).`;
  const policy = 'Policy (docs/CI.md, "When main breaks"): fix forward or revert within one working day.';
  if (!diagnostics || !Array.isArray(diagnostics.rows)) {
    return `${head}\n\nThe run left no ci-diagnostics report, so its jobs couldn't be classified; open the run to see why.\n\n${policy}\n`;
  }
  const rows = diagnostics.rows.filter((r) => !PASSING.has(r.status));
  const table = ['| Check | Job | Status | First error or reason |', '| --- | --- | --- | --- |', ...rows.map((r) => `| ${cell(r.check)} | ${cell(r.job)} | ${cell(r.status)} | ${cell(firstError(r))} |`)];
  return `${head}\n\n${table.join('\n')}\n\nThe full report is the run's \`ci-diagnostics\` artifact (docs/CI_DIAGNOSTICS.md).\n\n${policy}\n`;
}

/**
 * @param {{diagnostics: {ok: boolean, rows: object[]} | null, openIssue: number | null,
 *   run: {url: string, sha: string, ref: string}, title?: string}} input
 * @returns {{action: 'none'} | {action: 'open', title: string, body: string} |
 *   {action: 'comment' | 'close', issue: number, body: string}}
 */
export function trackingAction({ diagnostics, openIssue, run, title = TRACKING_TITLE }) {
  const green = Boolean(diagnostics?.ok);
  if (green) {
    if (!openIssue) return { action: 'none' };
    return { action: 'close', issue: openIssue, body: `\`main\` is green again at ${run.sha.slice(0, 7)} ([run](${run.url})). Closing.\n` };
  }
  const body = failureBody(diagnostics, run);
  return openIssue ? { action: 'comment', issue: openIssue, body } : { action: 'open', title, body };
}
