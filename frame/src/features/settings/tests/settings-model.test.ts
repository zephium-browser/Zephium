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

it.each([
  ["Linux", true],
  ["macOS", false],
  ["Windows", false],
] as const)("offers Graphics on %s: %s", async (platform, offered) => {
  vi.resetModules();
  vi.doMock("$shared/platform", () => ({
    IS_LINUX: platform === "Linux",
    IS_MAC: platform === "macOS",
    IS_WINDOWS: platform === "Windows",
  }));
  try {
    const model = await import("../lib/settings-model");
    const listed = model.shownSections().filter((section) => section.group === "app");
    expect(listed.map((section) => section.id)).toEqual(
      offered ? ["features", "shortcuts", "graphics", "about"] : ["features", "shortcuts", "about"],
    );
    for (const query of ["graphics", "rendering mode", "GPU", "graphics report"]) {
      expect(model.searchSettings(query).some((result) => result.section === "graphics")).toBe(
        offered,
      );
      expect(model.searchSections(query).some((section) => section.id === "graphics")).toBe(
        offered,
      );
    }
    if (offered)
      expect(model.searchSettings("rendering mode").map((result) => result.target)).toEqual([
        "graphics.mode",
      ]);
  } finally {
    vi.doUnmock("$shared/platform");
    vi.resetModules();
  }
});

it("offers SmartScreen search results only on Windows", async () => {
  for (const windows of [false, true]) {
    vi.resetModules();
    vi.doMock("$shared/platform", () => ({
      IS_WINDOWS: windows,
      IS_MAC: !windows,
      IS_LINUX: false,
    }));
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

it.each([true, false])(
  "About localizes update reasons and install timing (mac=%s)",
  async (mac) => {
    vi.resetModules();
    vi.doMock("$shared/platform", () => ({ IS_MAC: mac, IS_WINDOWS: !mac, IS_LINUX: false }));
    let reason: "previousInstallIncomplete" | "workNotSaved" | "activityCheckFailed" =
      "previousInstallIncomplete";
    vi.doMock("$domain/updates", () => ({
      updates: {
        status: () => ({ state: "ready", version: "1.0.5", retry_reason: reason }),
        pendingRelaunch: () => false,
      },
      releaseNotesUrl: () => "",
      systemUpdateTarget: () => null,
    }));
    try {
      const { render } = await import("svelte/server");
      const { default: AboutPage } = await import("../components/sections/AboutPage.svelte");
      for (const [value, message] of [
        [
          "previousInstallIncomplete",
          "The previous update did not finish. Your downloaded update is still ready to install.",
        ],
        ["activityCheckFailed", "Couldn’t check for active downloads. Try again."],
        [
          "workNotSaved",
          "Your work could not be saved. The update is still ready; try again after saving.",
        ],
      ] as const) {
        reason = value;
        const html = render(AboutPage).body;
        expect(html).toContain(message);
        expect(html).toContain(
          "New versions download in the background and install when you relaunch.",
        );
      }
    } finally {
      vi.doUnmock("$shared/platform");
      vi.doUnmock("$domain/updates");
      vi.resetModules();
    }
  },
);

it.each([
  ["macOS 26.1.0", true],
  ["Windows 11", false],
  [undefined, true],
] as const)(
  "update install timing uses native OS %s despite the user agent",
  async (os, windows) => {
    vi.resetModules();
    vi.doMock("$shared/platform", () => ({
      IS_WINDOWS: windows,
      IS_MAC: !windows,
      IS_LINUX: false,
    }));
    try {
      const { fields } = await import("../lib/catalog");
      expect(fields["updates.auto-check"].description(os)).toBe(
        os?.startsWith("macOS")
          ? "New versions download in the background and install when you relaunch or quit."
          : "New versions download in the background and install when you relaunch.",
      );
    } finally {
      vi.doUnmock("$shared/platform");
      vi.resetModules();
    }
  },
);
