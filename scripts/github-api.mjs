// Reads the GitHub REST API from a workflow, with the job's token (GITHUB_TOKEN) for the job's
// repository (GITHUB_REPOSITORY). Used by the release-candidate and release workflows' scripts.

/** The JSON at `path`, relative to the repository (`actions/workflows/main.yml/runs?…`). */
export async function repoApi(path) {
  const repo = process.env.GITHUB_REPOSITORY;
  const token = process.env.GITHUB_TOKEN;
  if (!repo || !token) throw new Error('GITHUB_REPOSITORY and GITHUB_TOKEN are needed to read the GitHub API');
  const url = `https://api.github.com/repos/${repo}/${path}`;
  const res = await fetch(url, { headers: { accept: 'application/vnd.github+json', authorization: `Bearer ${token}`, 'x-github-api-version': '2022-11-28' } });
  if (!res.ok) throw new Error(`${url} answered ${String(res.status)}: ${await res.text()}`);
  return res.json();
}

/** Appends `values` to the step's outputs ($GITHUB_OUTPUT), and prints them. */
export async function writeOutputs(values) {
  const { appendFileSync } = await import('node:fs');
  for (const [key, value] of Object.entries(values)) console.log(`${key}=${value}`);
  if (process.env.GITHUB_OUTPUT) appendFileSync(process.env.GITHUB_OUTPUT, Object.entries(values).map(([k, v]) => `${k}=${v}\n`).join(''));
}
