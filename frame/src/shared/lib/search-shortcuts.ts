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

export function shortcutPrefix(input: string, shortcuts: readonly Shortcut[]) {
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
