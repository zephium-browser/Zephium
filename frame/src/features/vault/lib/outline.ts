/** A heading in the right panel's outline. `target` is a line for Markdown or
 *  an opaque PDF destination the viewer resolves. */
export type OutlineItem = { title: string; level: number; target: unknown };

/** ATX headings outside fenced code blocks. */
export function markdownOutline(text: string): OutlineItem[] {
  const items: OutlineItem[] = [];
  let fence: string | null = null;
  for (const [line, content] of text.split(/\r?\n/u).entries()) {
    const marker = /^\s*(```|~~~)/u.exec(content)?.[1];
    if (marker) {
      fence = fence === marker ? null : (fence ?? marker);
      continue;
    }
    if (fence) continue;
    const heading = /^(#{1,6})\s+(.+?)\s*#*\s*$/u.exec(content);
    if (heading?.[1] && heading[2]) {
      items.push({ title: heading[2], level: heading[1].length, target: line });
    }
  }
  return items;
}

/** `[[target]]` and `[[target|label]]` links in a note. */
export function wikilinks(text: string): string[] {
  const found = new Set<string>();
  for (const match of text.matchAll(/\[\[([^\]|#]+)(?:#[^\]|]*)?(?:\|[^\]]*)?\]\]/gu)) {
    if (match[1]) found.add(match[1].trim());
  }
  return [...found];
}
