import { describe, expect, it, vi } from "vitest";
import { searchSections } from "../lib/settings-model";

describe("settings discovery", () => {
  it("finds nested preferences by their localized vocabulary", () => {
    expect(searchSections("interface language").map((section) => section.id)).toContain("general");
    expect(searchSections("motion").map((section) => section.id)).toContain("appearance");
    expect(searchSections("download").map((section) => section.id)).toContain("general");
  });
  it("matches every search word and distinguishes no results", () => {
    expect(searchSections("  WINDOW tint  ").map((section) => section.id)).toEqual(["appearance"]);
    expect(searchSections("accent downloads")).toEqual([]);
    expect(searchSections("unfindablephrase")).toEqual([]);
  });
});

it("finds the agent destinations without exposing unimplemented controls", async () => {
  const { searchSettings, searchSections, sections } = await import("../lib/settings-model");
  expect(sections.some((section) => String(section.id) === "plugins")).toBe(false);
  expect(searchSections("models").some((section) => section.id === "ai")).toBe(true);
  expect(searchSections("site access").some((section) => section.id === "sites")).toBe(true);
  expect(searchSections("MCP").some((section) => section.id === "mcp")).toBe(true);
  for (const query of ["API key", "Jev", "TypeSafe"]) {
    expect(searchSettings(query).map(({ section, target }) => ({ section, target }))).toEqual([
      { section: "ai", target: "work.decisions.key" },
    ]);
  }
  expect(searchSettings("Decisions").some((result) => result.section === "work")).toBe(false);
  expect(searchSettings("reset settings").some((result) => result.target === "about.reset")).toBe(
    true,
  );
});

it("offers Account through the identity card and search, outside the groups", async () => {
  const { groups, sections, searchSettings } = await import("../lib/settings-model");
  const account = sections.find((section) => String(section.id) === "account");
  expect(account).toBeDefined();
  expect(groups.some((group) => String(group.id) === account!.group)).toBe(false);
  expect(searchSettings("account").some((result) => String(result.section) === "account")).toBe(
    true,
  );
});

it("groups sections as Browser, Work and App with stable ids", async () => {
  const { groups, sections } = await import("../lib/settings-model");
  const listed = (group: string) =>
    sections.filter((section) => section.group === group).map((section) => section.id);
  expect(groups.map((group) => group.id)).toEqual(["browser", "work", "app"]);
  expect(listed("browser").filter((id) => id !== "profiles")).toEqual([
    "general",
    "appearance",
    "tabs",
    "newtab",
    "search",
    "privacy",
    "focus",
    "performance",
  ]);
  expect(listed("work")).toEqual(["work", "ai", "skills", "memory", "sites", "mcp"]);
  expect(listed("app")).toEqual(["features", "shortcuts", "about"]);
});

it("finds every real tab option", async () => {
  const { searchSettings } = await import("../lib/settings-model");
  for (const [query, target] of [
    ["archive", "tabs.archive"],
    ["other apps", "tabs.external"],
    ["close buttons", "tabs.close-button"],
    ["login", "general.login"],
  ]) {
    expect(searchSettings(query!).some((result) => result.target === target)).toBe(true);
  }
});

it("finds the page zoom options", async () => {
  const { searchSettings } = await import("../lib/settings-model");
  for (const [query, target] of [
    ["default zoom", "appearance.zoom"],
    ["zoom levels", "appearance.zoom-levels"],
  ]) {
    expect(searchSettings(query!).some((result) => result.target === target)).toBe(true);
  }
});

it("takes each feature's pages out of navigation and search while its switch is off", async () => {
  const off = new Set<string>();
  vi.resetModules();
  vi.doMock("$domain/preferences", async (actual) => {
    const real = (await actual()) as typeof import("$domain/preferences");
    return {
      preferences: {
        ...real.preferences,
        aiEnabled: () => !off.has("ai"),
        enabled: (feature: string) => !off.has(feature),
      },
    };
  });
  const { shownSections, searchSettings, searchSections } = await import("../lib/settings-model");
  const shown = () => shownSections().map((section) => section.id);
  expect(shown()).toEqual(expect.arrayContaining(["work", "ai", "focus", "features"]));
  off.add("ai");
  const gone = new Set(["ai", "skills", "memory", "sites", "mcp"]);
  expect(shown().filter((id) => gone.has(id))).toEqual([]);
  expect(shown()).toContain("work");
  for (const query of ["models", "skills", "memory", "site access", "MCP", "API key", "dictation"])
    expect(searchSettings(query).some((result) => gone.has(result.section))).toBe(false);
  expect(searchSections("models").some((section) => gone.has(section.id))).toBe(false);
  off.add("work");
  off.add("time");
  expect(shown()).not.toContain("work");
  expect(shown()).not.toContain("focus");
  expect(searchSettings("AI").map((result) => result.target)).toContain("ai.enabled");
  vi.doUnmock("$domain/preferences");
});

it("offers SmartScreen search results only on Windows", async () => {
  for (const windows of [false, true]) {
    vi.resetModules();
    vi.doMock("$shared/platform", () => ({ IS_WINDOWS: windows }));
    const model = await import("../lib/settings-model");
    expect(
      model.searchSettings("SmartScreen").some((field) => field.target === "privacy.smartscreen"),
    ).toBe(windows);
    expect(model.searchSections("SmartScreen").some((section) => section.id === "privacy")).toBe(
      windows,
    );
  }
  vi.doUnmock("$shared/platform");
});
