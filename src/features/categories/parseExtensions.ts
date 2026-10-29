/** The extensions typed for a category, as Rust keeps them: lowercase, without the dot, each once (V1's rule). */
export function parseExtensions(text: string): string[] {
  return [
    ...new Set(
      text
        .split(/[\s,;]+/)
        .map((e) => e.trim().replace(/^\.+/, '').toLowerCase())
        .filter(Boolean),
    ),
  ];
}
