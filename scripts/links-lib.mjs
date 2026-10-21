// Pure logic behind `pnpm check:links`: finds relative Markdown links whose file or heading anchor
// doesn't exist. External links (http, https, mailto) are not fetched. Code spans and fenced code
// blocks are ignored. The CLI (check-links.mjs) supplies the file reads; tests use an in-memory tree.
import { posix } from 'node:path';

/** GitHub's anchor for a heading: lowercase, punctuation removed, spaces to hyphens. */
export function githubSlug(heading) {
  return heading
    .trim()
    .toLowerCase()
    .replace(/[^\p{L}\p{N}\s_-]/gu, '')
    .replace(/\s/g, '-');
}

const withoutCode = (md) => md.replace(/^(```|~~~)[\s\S]*?^\1/gm, '').replace(/`[^`\n]*`/g, '');

/** Anchors of every heading in a Markdown file, numbered like GitHub for repeated headings. */
function anchorsOf(md) {
  const seen = new Map();
  const anchors = new Set();
  for (const m of withoutCode(md).matchAll(/^#{1,6}\s+(.+?)\s*#*\s*$/gm)) {
    const slug = githubSlug(m[1]);
    const n = seen.get(slug) ?? 0;
    anchors.add(n ? `${slug}-${n}` : slug);
    seen.set(slug, n + 1);
  }
  return anchors;
}

const isExternal = (target) => /^[a-z][a-z0-9+.-]*:/i.test(target) || target.startsWith('//');

/**
 * @param {string[]} files repository-relative Markdown paths to check (forward slashes)
 * @param {(path: string) => string | undefined} read file contents, or undefined if it doesn't exist
 * @param {(path: string) => boolean} [isDir] whether a directory exists (for links ending in `/`)
 * @returns {{file: string, link: string, problem: 'missing file' | 'missing anchor'}[]}
 */
export function brokenLinks(files, read, isDir = () => false) {
  const broken = [];
  for (const file of files) {
    const md = read(file) ?? '';
    for (const m of withoutCode(md).matchAll(/\]\(\s*<?([^)\s>]+)>?(?:\s+"[^"]*")?\s*\)/g)) {
      const link = m[1];
      if (isExternal(link)) continue;
      const [rawPath, anchor] = link.split('#');
      const target = rawPath ? posix.normalize(posix.join(posix.dirname(file), decodeURIComponent(rawPath))) : file;
      if (rawPath && rawPath.endsWith('/')) {
        if (!isDir(target)) broken.push({ file, link, problem: 'missing file' });
        continue;
      }
      const content = read(target);
      if (content === undefined) {
        if (!isDir(target)) broken.push({ file, link, problem: 'missing file' });
        continue;
      }
      if (anchor && target.endsWith('.md') && !anchorsOf(content).has(anchor)) {
        broken.push({ file, link, problem: 'missing anchor' });
      }
    }
  }
  return broken;
}
