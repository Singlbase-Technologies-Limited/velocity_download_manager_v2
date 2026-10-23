// Pure logic behind CI failure collection (#91, docs/CI_DIAGNOSTICS.md):
// - extractErrors: turns a check's output into {file, line, col, test, message} entries.
// - buildDiagnostics: classifies every check of a run (PASSED, FAILED, BLOCKED,
//   SKIPPED-BY-SELECTOR, INFRA, CANCELLED) and decides whether the run passes. It never passes
//   when a mandatory job failed, was cancelled, or was skipped without a selector decision.
// - diagnosticsMarkdown: the run-summary table.
// No I/O here; the CLIs (ci-job-report.mjs, ci-summary.mjs) supply the inputs.

// Tool output contains terminal colour codes (ESC [ … m); matching the ESC control character is
// the point of this pattern, which the rule's intent (catching accidental control characters) excludes.
// eslint-disable-next-line no-control-regex -- deliberately matches ESC to strip ANSI colour codes
const ANSI = /\u001b\[[0-9;]*m/g;
const TOP_LEVEL = /\/(crates|src-tauri|src|scripts|docs|public|\.github|\.claude)\//;

/** Repository-relative path with forward slashes, from absolute or relative Windows/Unix paths. */
function relativePath(raw, root) {
  let p = raw.trim().replace(/^\\\\\?\\/, '').replaceAll('\\', '/');
  const r = (root ?? '').replace(/^\\\\\?\\/, '').replaceAll('\\', '/').replace(/\/$/, '');
  if (r && p.toLowerCase().startsWith(`${r.toLowerCase()}/`)) return p.slice(r.length + 1);
  if (/^([a-z]:)?\//i.test(p)) {
    const m = TOP_LEVEL.exec(p);
    if (m) p = p.slice(m.index + 1);
  }
  return p.replace(/^\.\//, '');
}

const SUMMARY_LINES = /could not compile|build failed|aborting due to|test failed, to rerun|targets? failed|^error: \d+ targets? failed/;

function compilerErrors(lines, root) {
  const out = [];
  for (let i = 0; i < lines.length; i++) {
    const m = /^(error(\[E\d+\])?): (.*)$/.exec(lines[i]);
    if (!m || SUMMARY_LINES.test(lines[i])) continue;
    for (let j = i + 1; j < Math.min(i + 4, lines.length); j++) {
      const loc = /^\s*--> (.+):(\d+):(\d+)/.exec(lines[j]);
      if (loc) {
        out.push({ file: relativePath(loc[1], root), line: Number(loc[2]), col: Number(loc[3]), message: m[2] ? `${m[1]}: ${m[3]}` : m[3] });
        break;
      }
    }
  }
  return out;
}

const parsers = {
  'rust-fmt': (lines, root) =>
    lines.flatMap((l) => {
      const m = /^Diff in (.+):(\d+):\s*$/.exec(l);
      return m ? [{ file: relativePath(m[1], root), line: Number(m[2]), message: 'Formatting differs from rustfmt' }] : [];
    }),
  'rust-clippy': compilerErrors,
  'rust-release': compilerErrors,
  'rust-test': (lines, root) => {
    const out = compilerErrors(lines, root);
    for (let i = 0; i < lines.length; i++) {
      const t = /^---- (.+) stdout ----$/.exec(lines[i]);
      if (!t) continue;
      for (let j = i + 1; j < lines.length && !lines[j].startsWith('---- '); j++) {
        const p = /panicked at (.+):(\d+):(\d+):$/.exec(lines[j]);
        if (p) {
          out.push({ file: relativePath(p[1], root), line: Number(p[2]), col: Number(p[3]), test: t[1], message: (lines[j + 1] ?? '').trim() });
          break;
        }
      }
    }
    return out;
  },
  tsc: (lines, root) =>
    lines.flatMap((l) => {
      const m = /^(.+)\((\d+),(\d+)\): error (TS\d+: .*)$/.exec(l);
      return m ? [{ file: relativePath(m[1], root), line: Number(m[2]), col: Number(m[3]), message: m[4] }] : [];
    }),
  eslint: (lines, root) => {
    const out = [];
    let file;
    for (const l of lines) {
      const e = /^\s+(\d+):(\d+)\s+error\s+(.+?)\s{2,}(\S+)\s*$/.exec(l);
      if (e && file) out.push({ file, line: Number(e[1]), col: Number(e[2]), message: `${e[3]} (${e[4]})` });
      else if (/^\S/.test(l) && !l.startsWith('✖')) file = relativePath(l, root);
    }
    return out;
  },
  vitest: (lines, root) => {
    const out = [];
    for (let i = 0; i < lines.length; i++) {
      const f = /^ FAIL {2}(.+?) > (.+)$/.exec(lines[i]);
      if (!f) continue;
      const entry = { file: relativePath(f[1], root), test: f[2], message: '' };
      for (let j = i + 1; j < lines.length && !/^ FAIL {2}/.test(lines[j]); j++) {
        if (!entry.message && lines[j].trim()) entry.message = lines[j].trim();
        const loc = /^ ❯ (.+?):(\d+):(\d+)/.exec(lines[j]);
        if (loc) {
          Object.assign(entry, { file: relativePath(loc[1], root), line: Number(loc[2]), col: Number(loc[3]) });
          break;
        }
      }
      const { file, line, col, test, message } = entry;
      out.push(line ? { file, line, col, test, message } : { file, test, message });
    }
    return out;
  },
  links: (lines) =>
    lines.flatMap((l) => {
      const m = /^BROKEN {2}(.+?): (.+)$/.exec(l);
      return m ? [{ file: m[1], message: m[2] }] : [];
    }),
};

/**
 * @param {string} check the check id (rust-fmt, rust-clippy, rust-test, tsc, eslint, vitest, links…)
 * @param {string} output what the check printed (stdout and stderr)
 * @param {{root?: string, max?: number}} [options]
 */
export function extractErrors(check, output, { root, max = 10 } = {}) {
  const lines = output.replace(ANSI, '').replaceAll('\r', '').split('\n');
  let errors = (parsers[check] ?? (() => []))(lines, root);
  if (errors.length === 0) {
    const tail = lines.filter((l) => l.trim()).slice(-10);
    return tail.length ? [{ message: tail.join('\n') }] : [];
  }
  if (errors.length > max) errors = [...errors.slice(0, max), { message: `${errors.length - max} more not shown; see the job log` }];
  return errors;
}

/**
 * @param {{
 *   needs: Record<string, {result: string, outputs?: Record<string, string>}>,
 *   reports: Record<string, {job: string, infraFailure: string | null, checks: {id: string, name: string, status: 'pass' | 'fail', errors: object[]}[]}>,
 *   jobs: {id: string, name?: string, mandatory: boolean, selectedBy?: string, selectedOutput?: string}[],
 * }} input
 * @returns {{ok: boolean, rows: {check: string, job: string, status: string, reason?: string, errors: object[]}[]}}
 */
export function buildDiagnostics({ needs, reports, jobs }) {
  const rows = [];
  const jobRow = (job, status, reason) => rows.push({ check: job.name ?? job.id, job: job.id, status, reason, errors: [] });

  for (const job of jobs) {
    const result = needs[job.id]?.result ?? 'skipped';
    const report = reports[job.id];

    if (result === 'skipped') {
      const sel = job.selectedBy ? needs[job.selectedBy] : undefined;
      if (sel?.result === 'success' && sel.outputs?.[job.selectedOutput] === 'false') {
        jobRow(job, 'SKIPPED-BY-SELECTOR', `not needed for this change (decided by ${job.selectedBy})`);
      } else if (sel && sel.result !== 'success') {
        jobRow(job, 'BLOCKED', `prerequisite ${job.selectedBy} did not succeed (${sel.result})`);
      } else {
        jobRow(job, 'BLOCKED', 'skipped although no selector decision deselected it');
      }
      continue;
    }
    if (result === 'cancelled') {
      jobRow(job, 'CANCELLED', 'the job was cancelled');
      continue;
    }
    if (report?.infraFailure) {
      jobRow(job, 'INFRA', `setup step "${report.infraFailure}" failed before any check ran; a re-run candidate`);
      continue;
    }
    if (!report) {
      if (result === 'success') jobRow(job, 'PASSED');
      else jobRow(job, 'INFRA', `the job ended with "${result}" before writing its report`);
      continue;
    }
    for (const c of report.checks) {
      rows.push({ check: c.name, job: job.id, status: c.status === 'pass' ? 'PASSED' : 'FAILED', errors: c.status === 'pass' ? [] : c.errors });
    }
    if (result !== 'success' && !report.checks.some((c) => c.status !== 'pass')) {
      jobRow(job, 'INFRA', `the job ended with "${result}" although every check passed`);
    }
  }

  const passing = new Set(['PASSED', 'SKIPPED-BY-SELECTOR']);
  const mandatory = new Set(jobs.filter((j) => j.mandatory).map((j) => j.id));
  const ok = rows.every((r) => passing.has(r.status) || !mandatory.has(r.job));
  return { ok, rows };
}

const cell = (s) => String(s ?? '').replaceAll('|', '\\|').replaceAll('\n', ' ');
const where = (e) => (e.file ? `\`${e.file}${e.line ? `:${e.line}` : ''}${e.col ? `:${e.col}` : ''}\` ` : '');

/** Markdown for the run summary: one row per check, with the first three errors. */
export function diagnosticsMarkdown(diagnostics) {
  const rows = diagnostics.rows.map((r) => {
    const details = r.reason ?? r.errors.slice(0, 3).map((e) => `${where(e)}${e.test ? `${e.test}: ` : ''}${e.message}`).join('<br>');
    return `| ${cell(r.check)} | ${r.job} | ${r.status} | ${cell(details)} |`;
  });
  return [
    '### CI diagnostics',
    '',
    `Result: **${diagnostics.ok ? 'passed' : 'failed'}**. All errors are in the \`ci-diagnostics\` artifact (docs/CI_DIAGNOSTICS.md).`,
    '',
    '| Check | Job | Status | Details |',
    '| --- | --- | --- | --- |',
    ...rows,
    '',
  ].join('\n');
}

/**
 * Each job's report from its newest attempt. A re-run of failed jobs leaves the earlier attempts'
 * reports in the run, so without this a job that passed when re-run still counted as failed. A
 * report without an attempt (written before #46) counts as the first.
 */
export function latestReports(list) {
  const latest = {};
  for (const r of list) {
    const kept = latest[r.job];
    if (!kept || (r.attempt ?? 1) > (kept.attempt ?? 1)) latest[r.job] = r;
  }
  return latest;
}
