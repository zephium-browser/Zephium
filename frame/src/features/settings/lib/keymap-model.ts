import * as m from "$shared/i18n/messages";
import type { KeymapEntry } from "$domain/keymap";

/** Native titles are English menu labels; the frame shows its own, so they
 *  translate with the rest of the interface. Native's title is the fallback
 *  for a command added before its string. */
const TITLES: Record<string, () => string> = {
  "browser.settings": m.command_browser_settings,
  "tab.new": m.command_tab_new,
  "window.newPrivate": m.command_window_new_private,
  "window.closePrivate": m.command_window_close_private,
  "note.new": m.command_note_new,
  "split.choose": m.command_split_choose,
  "page.print": m.command_page_print,
  "page.devtools": m.command_page_devtools,
  "tab.close": m.command_tab_close,
  "page.copyLink": m.command_page_copy_link,
  "find.show": m.command_find_show,
  "find.next": m.command_find_next,
  "find.previous": m.command_find_previous,
  "sidebar.toggleCompact": m.command_sidebar_compact,
  "nav.reload": m.command_nav_reload,
  "nav.stop": m.command_nav_stop,
  "url.focus": m.command_url_focus,
  "zoom.in": m.command_zoom_in,
  "zoom.out": m.command_zoom_out,
  "zoom.reset": m.command_zoom_reset,
  "nav.back": m.command_nav_back,
  "nav.forward": m.command_nav_forward,
  "tab.reopen": m.command_tab_reopen,
  "browser.history": m.command_browser_history,
  "tab.next": m.command_tab_next,
  "tab.previous": m.command_tab_previous,
  "tool.downloads": m.command_tool_downloads,
  "bookmark.add": m.command_bookmark_add,
  "tool.bookmarks": m.command_tool_bookmarks,
  "browser.tasks": m.command_browser_tasks,
  "browser.notes": m.command_browser_notes,
  "settings.shortcuts": m.command_settings_shortcuts,
  "tab.select.last": m.command_tab_select_last,
  "theme.system": m.command_theme_system,
  "theme.light": m.command_theme_light,
  "theme.dark": m.command_theme_dark,
};

export function commandTitle(entry: Pick<KeymapEntry, "id" | "title">): string {
  const position = /^tab\.select\.(\d)$/u.exec(entry.id)?.[1];
  if (position) return m.command_tab_select({ number: position });
  return TITLES[entry.id]?.() ?? entry.title;
}

export type KeymapSection = { id: string; title: string; entries: KeymapEntry[] };

const tabCommand = (entry: KeymapEntry) => entry.id.startsWith("tab.");

const SECTIONS: { id: string; title: () => string; holds: (entry: KeymapEntry) => boolean }[] = [
  {
    id: "browser",
    title: m.keymap_group_browser,
    holds: (entry) =>
      ["app", "file", "edit", "bookmarks", "help"].includes(entry.group) ||
      (entry.group === "window" && !tabCommand(entry)),
  },
  {
    id: "tabs",
    title: m.keymap_group_tabs,
    holds: (entry) => (entry.group === "window" || entry.group === "keys") && tabCommand(entry),
  },
  { id: "view", title: m.keymap_group_view, holds: (entry) => entry.group === "view" },
  { id: "history", title: m.keymap_group_history, holds: (entry) => entry.group === "history" },
  {
    id: "appearance",
    title: m.keymap_group_appearance,
    holds: (entry) => entry.group === "global",
  },
];

/** The rebindable commands, in the order Keyboard settings lists them. The
 *  launcher has its own recorder and Work keys are fixed to the pane. */
export function keymapSections(entries: readonly KeymapEntry[]): KeymapSection[] {
  return SECTIONS.map((section) => ({
    id: section.id,
    title: section.title(),
    entries: entries.filter((entry) => entry.customizable && section.holds(entry)),
  })).filter((section) => section.entries.length > 0);
}
