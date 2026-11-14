import type { SearchShortcut } from "$shared/ipc/bindings";

export function shortcutKeywords(value: string, id: string, entries: readonly SearchShortcut[]) {
  const keywords = value.trim().split(/\s+/u);
  const used = new Set(
    entries
      .filter((entry) => entry.id !== id)
      .flatMap((entry) => entry.keywords.map((key) => key.toLowerCase())),
  );
  if (keywords.length > 8 || keywords.some((key) => !/^[a-z0-9_-]{1,32}$/iu.test(key))) return null;
  for (const key of keywords) {
    if (used.has(key.toLowerCase())) return null;
    used.add(key.toLowerCase());
  }
  return keywords;
}

export function customShortcut(
  name: string,
  keyword: string,
  template: string,
  entries: readonly SearchShortcut[],
  id: string,
): SearchShortcut | null {
  const keywords = shortcutKeywords(keyword, id, entries);
  if (
    !name.trim() ||
    name.length > 80 ||
    Array.from(name).some((character) => {
      const code = character.charCodeAt(0);
      return code < 32 || (code >= 127 && code <= 159);
    }) ||
    !keywords ||
    template.length > 2048 ||
    template.split("{query}").length !== 2
  )
    return null;
  try {
    const url = new URL(template.replace("{query}", "zephium-query"));
    if (
      url.protocol !== "https:" ||
      url.username ||
      url.password ||
      url.hash ||
      url.hostname.includes("zephium-query") ||
      !(url.pathname.includes("zephium-query") || url.search.includes("zephium-query"))
    )
      return null;
    return {
      id,
      name: name.trim(),
      keywords,
      search_url: template,
      home_url: `${url.origin}/`,
      domain: url.hostname,
      enabled: true,
      suggestions: null,
    };
  } catch {
    return null;
  }
}
