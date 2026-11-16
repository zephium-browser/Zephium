import type { SearchShortcut } from "$shared/ipc/bindings";

type Shortcut = Pick<SearchShortcut, "id" | "name" | "keywords" | "domain" | "enabled">;

const prefixStoplist = new Set([
  "a",
  "i",
  "e",
  "x",
  "r",
  "b",
  "d",
  "g",
  "k",
  "w",
  "so",
  "aw",
  "gm",
  "gl",
  "gr",
  "tw",
  "wa",
  "ia",
  "tr",
  "sp",
  "ok",
  "hi",
  "no",
  "yo",
  "go",
  "to",
  "in",
  "on",
  "at",
  "it",
  "is",
  "me",
  "my",
  "we",
  "us",
  "or",
  "an",
  "as",
  "be",
  "do",
  "he",
  "if",
  "of",
  "up",
  "ya",
  "lol",
  "omg",
  "idk",
  "brb",
  "btw",
  "hey",
  "yes",
  "google",
  "brave",
  "perplexity",
  "wiki",
  "images",
  "maps",
  "twitter",
  "crates",
  "crate",
  "translate",
  "rust",
  "twitch",
  "archive",
]);

export function shortcutPrefix<T extends Shortcut>(input: string, shortcuts: readonly T[]) {
  const space = input.indexOf(" ");
  if (space < 2) return null;
  const keyword = input.slice(0, space).toLowerCase();
  if (prefixStoplist.has(keyword)) return null;
  const shortcut = shortcuts.find(
    (entry) => entry.enabled && entry.keywords.some((key) => key.toLowerCase() === keyword),
  );
  return shortcut ? { shortcut, query: input.slice(space + 1).trim() } : null;
}

export function emptyShortcutBackspace(input: string, shortcuts: readonly Shortcut[]) {
  const parsed = shortcutPrefix(input, shortcuts);
  return parsed && !parsed.query ? "" : null;
}

/** The shortcut a query is headed for, as native reads it: a keyword before
 *  the first space, else a bang anywhere in it. */
export function shortcutFor<T extends Shortcut>(input: string, shortcuts: readonly T[]): T | null {
  const prefixed = shortcutPrefix(input, shortcuts);
  if (prefixed) return prefixed.shortcut;
  for (const token of input.split(/\s+/u)) {
    if (!token.startsWith("!")) continue;
    const key = token.slice(1).toLowerCase();
    const found = shortcuts.find(
      (entry) => entry.enabled && entry.keywords.some((keyword) => keyword.toLowerCase() === key),
    );
    if (found) return found;
  }
  return null;
}

/** How a shortcut is offered: as a bang on its first keyword, which reaches
 *  it from anywhere in a query, so every one is typed the same way. */
export function shortcutKeyword(shortcut: Shortcut): string {
  return `!${shortcut.keywords[0] ?? ""}`;
}

/** The query with whatever shortcut it named taken out, and `keyword` put in
 *  front of what is left. */
export function withShortcut(input: string, keyword: string, shortcuts: readonly Shortcut[]) {
  const prefixed = shortcutPrefix(input, shortcuts);
  const rest = prefixed
    ? prefixed.query
    : input
        .split(/\s+/u)
        .filter(
          (token) =>
            !token.startsWith("!") ||
            !shortcuts.some(
              (entry) =>
                entry.enabled &&
                entry.keywords.some((key) => key.toLowerCase() === token.slice(1).toLowerCase()),
            ),
        )
        .join(" ")
        .trim();
  return rest ? `${keyword} ${rest}` : `${keyword} `;
}
