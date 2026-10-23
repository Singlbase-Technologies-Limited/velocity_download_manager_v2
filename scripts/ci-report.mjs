#!/usr/bin/env node
// CI failure collection (#91, docs/CI_DIAGNOSTICS.md). Two subcommands:
//
//   node scripts/ci-report.mjs job --job <id> --infra <step ids> [--step-check <id>=<name> …]
//     Run as a job's last step (`if: ${{ !cancelled() }}`). Reads the step outcomes from $STEPS
//     (`toJSON(steps)`) and the checks the job recorded in $CI_REPORT_PATH, and writes
//     ci-reports/report-<job>.json, with the run attempt. A failed infra step (checkout, toolchain, cache, install) marks
//     the job INFRA; a failed --step-check step that recorded nothing becomes a failed check.
//
//   node scripts/ci-report.mjs summary --jobs '<json>' [--reports <dir>]
//     Run as the final summary step. Reads the job results from $NEEDS (`toJSON(needs)`) and the
//     job reports (each job's newest attempt, from any folder under --reports), writes the table to $GITHUB_STEP_SUMMARY and ci-diagnostics.json, and exits
//     non-zero unless every mandatory check passed or was deselected by the selector.
//
// The decisions live in diagnostics-lib.mjs, which is unit tested.
import { existsSync, mkdirSync, readFileSync, readdirSync, writeFileSync, appendFileSync } from 'node:fs';
import { basename, join } from 'node:path';
import { buildDiagnostics, diagnosticsMarkdown, latestReports } from './diagnostics-lib.mjs';

const [command, ...rest] = process.argv.slice(2);
const opts = {};
for (let i = 0; i < rest.length; i += 2) {
  const key = rest[i]?.replace(/^--/, '');
  if (!key || rest[i + 1] === undefined) {
    console.error(`Missing value for ${rest[i]}`);
    process.exit(2);
  }
  (opts[key] ??= []).push(rest[i + 1]);
}
const one = (k) => opts[k]?.[0];
const readJson = (text, what) => {
  try {
    return JSON.parse(text);
  } catch {
    console.error(`${what} is not valid JSON`);
    process.exit(2);
  }
};

if (command === 'job') {
  const job = one('job');
  if (!job) {
    console.error('Usage: ci-report.mjs job --job <id> --infra <ids> [--step-check <id>=<name>]');
    process.exit(2);
  }
  const steps = readJson(process.env.STEPS ?? '{}', '$STEPS');
  const infraIds = (one('infra') ?? '').split(',').filter(Boolean);
  const infraFailure = infraIds.find((id) => steps[id]?.outcome === 'failure') ?? null;
  const path = process.env.CI_REPORT_PATH;
  const recorded = path && existsSync(path) ? readJson(readFileSync(path, 'utf8'), path).checks : [];
  const checks = [...recorded];
  // A failed check step usually failed because a check it ran failed, which is already recorded.
  // Only when nothing recorded a failure (the step crashed, or wrote no report) is the step itself
  // reported as the failed check, so a failure is never lost.
  const recordedFailure = recorded.some((c) => c.status === 'fail');
  for (const spec of opts['step-check'] ?? []) {
    const [id, name] = spec.split('=');
    if (steps[id]?.outcome === 'failure' && !recordedFailure && !checks.some((c) => c.id === id)) {
      checks.push({ id, name: name ?? id, status: 'fail', errors: [{ message: `Step "${id}" failed; see the job log` }] });
    }
  }
  mkdirSync('ci-reports', { recursive: true });
  const out = join('ci-reports', `report-${job}.json`);
  const attempt = Number(process.env.GITHUB_RUN_ATTEMPT ?? 1);
  writeFileSync(out, JSON.stringify({ job, attempt, infraFailure, checks }, null, 2));
  console.log(`Wrote ${out}: ${checks.length} check(s), infra failure: ${infraFailure ?? 'none'}`);
} else if (command === 'summary') {
  const jobs = readJson(one('jobs') ?? '', '--jobs');
  const needs = readJson(process.env.NEEDS ?? '{}', '$NEEDS');
  const dir = one('reports') ?? 'ci-reports';
  // Each attempt's reports are in a folder of their own (report-<job>-<attempt>), so a re-run
  // never hides behind the attempt it replaced (#46).
  const found = existsSync(dir)
    ? readdirSync(dir, { recursive: true })
        .map(String)
        .filter((n) => /^report-.+\.json$/.test(basename(n)))
    : [];
  const reports = latestReports(found.map((f) => readJson(readFileSync(join(dir, f), 'utf8'), f)));
  const diagnostics = buildDiagnostics({ needs, reports, jobs });
  const { GITHUB_SERVER_URL: server, GITHUB_REPOSITORY: repo, GITHUB_RUN_ID: runId } = process.env;
  const run = {
    id: runId ?? null,
    url: server && repo && runId ? `${server}/${repo}/actions/runs/${runId}` : null,
    sha: process.env.GITHUB_SHA ?? null,
    event: process.env.GITHUB_EVENT_NAME ?? null,
    workflow: process.env.GITHUB_WORKFLOW ?? null,
  };
  writeFileSync('ci-diagnostics.json', JSON.stringify({ schema: 1, run, ...diagnostics }, null, 2));
  const md = diagnosticsMarkdown(diagnostics);
  if (process.env.GITHUB_STEP_SUMMARY) appendFileSync(process.env.GITHUB_STEP_SUMMARY, md);
  console.log(md);
  process.exit(diagnostics.ok ? 0 : 1);
} else {
  console.error('Usage: ci-report.mjs job|summary …');
  process.exit(2);
}
