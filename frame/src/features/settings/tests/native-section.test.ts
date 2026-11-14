import { expect, test } from "vitest";
import { handleNativeSection, section } from "../lib/settings-state.svelte";

test("every settings page a menu or command names is the one that opens", () => {
  for (const id of [
    "general",
    "about",
    "shortcuts",
    "account",
    "profiles",
    "newtab",
    "ai",
    "sites",
    "focus",
    "appearance",
    "tabs",
    "performance",
    "work",
    "skills",
    "memory",
    "mcp",
    "features",
    "search",
    "privacy",
    "graphics",
  ]) {
    handleNativeSection(`settings.section.${id}`);
    expect(section()).toBe(id);
  }
  handleNativeSection("settings.section.unknown");
  expect(section()).toBe("graphics");
});

test("Search opens at Search whichever page Settings was last left on", () => {
  handleNativeSection("settings.section.appearance");
  handleNativeSection("settings.section.search");
  expect(section()).toBe("search");
});
