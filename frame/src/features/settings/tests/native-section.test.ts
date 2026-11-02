import { expect, test } from "vitest";
import { handleNativeSection, section } from "../lib/settings-state.svelte";

test("every settings page a menu or command names is the one that opens", () => {
  for (const id of [
    "about",
    "shortcuts",
    "account",
    "profiles",
    "newtab",
    "ai",
    "sites",
    "focus",
    "privacy",
  ]) {
    handleNativeSection(`settings.section.${id}`);
    expect(section()).toBe(id);
  }
  handleNativeSection("settings.section.unknown");
  expect(section()).toBe("privacy");
});
