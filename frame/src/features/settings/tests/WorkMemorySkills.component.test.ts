import { afterEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page, userEvent } from "vitest/browser";
import MemoryPage from "../components/sections/MemoryPage.svelte";
import SkillsPage from "../components/sections/SkillsPage.svelte";

const PROFILE = "00000000000000000000000001";
const native = vi.hoisted(() => ({ memory: vi.fn(), skill: vi.fn() }));
vi.mock("$domain/tabs", () => ({
  tabs: { profile: () => ({ id: "00000000000000000000000001", kind: "regular" }) },
}));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  const { memories, skills, skillText } = await import("./work-context-fixtures");
  const memory = (profile: string) => ({
    version: 1,
    profile,
    memories,
    refused: null,
    error: null,
  });
  const listed = (profile: string, text: string | null = null) => ({
    version: 1,
    profile,
    skills,
    text,
    fault: null,
    error: null,
  });
  native.memory.mockImplementation(async (profile: string) => memory(profile));
  native.skill.mockImplementation(async (profile: string) => listed(profile));
  return mockBindings({
    workMemories: vi.fn(async (profile: string) => memory(profile)),
    workChangeMemory: native.memory,
    workSkills: vi.fn(async (profile: string) => listed(profile)),
    workSkillText: vi.fn(async (profile: string, name: string) => listed(profile, skillText(name))),
    workChangeSkill: native.skill,
  });
});

test("a fact is edited in place, added, forgotten, and everything can be forgotten after a confirm", async () => {
  await page.viewport(1000, 900);
  const screen = await render(MemoryPage);
  await screen.getByRole("button", { name: /Prefers aisle seats/u }).click();
  const field = screen.getByRole("textbox", { name: "Edit" });
  await field.fill("Prefers window seats on long flights");
  await userEvent.keyboard("{Enter}");
  expect(native.memory).toHaveBeenLastCalledWith(
    PROFILE,
    { query: null, work: null },
    {
      kind: "edit",
      id: "01M3F2YJTVB6S1M73MERQER7NA",
      text: "Prefers window seats on long flights",
      memory: "preference",
    },
  );
  expect(
    native.memory.mock.calls.filter((call) => (call[2] as { kind: string }).kind === "edit"),
  ).toHaveLength(1);
  await screen
    .getByRole("textbox", { name: "Something for the agent to remember" })
    .fill("Lives in Warsaw");
  await screen.getByRole("button", { name: "Add", exact: true }).click();
  expect(native.memory).toHaveBeenLastCalledWith(
    PROFILE,
    { query: null, work: null },
    { kind: "add", text: "Lives in Warsaw", memory: "fact" },
  );
  await screen.getByRole("button", { name: "Forget", exact: true }).first().click();
  expect(native.memory).toHaveBeenLastCalledWith(
    PROFILE,
    { query: null, work: null },
    { kind: "forget", id: "01M3F2YJTVB6S1M73MERQER7NA" },
  );
  await screen.getByRole("button", { name: "Forget everything…" }).click();
  await screen.getByRole("button", { name: "Forget everything", exact: true }).click();
  expect(native.memory).toHaveBeenLastCalledWith(
    PROFILE,
    { query: null, work: null },
    { kind: "forget_all" },
  );
});

test("skills turn off, a person's skill is saved from its fields, and a customised one goes back", async () => {
  await page.viewport(1000, 1400);
  const screen = await render(SkillsPage);
  await screen.getByRole("switch", { name: "Use Trip planning" }).click();
  expect(native.skill).toHaveBeenLastCalledWith(PROFILE, {
    kind: "set_enabled",
    name: "trip-planning",
    enabled: false,
  });
  await screen.getByRole("button", { name: /Weekly review/u }).click();
  await screen.getByRole("textbox", { name: "Name" }).fill("Friday review");
  await screen.getByRole("button", { name: "Save", exact: true }).click();
  const saved = native.skill.mock.lastCall![1] as { kind: string; previous: string; text: string };
  expect(saved.kind).toBe("save");
  expect(saved.previous).toBe("weekly-review");
  expect(saved.text).toContain("name: friday-review\n");
  expect(saved.text).toContain("role: light\n");
  expect(saved.text).toContain("## Result");
  await screen.getByRole("button", { name: /Job search/u }).click();
  await screen.getByRole("button", { name: "Use the built-in" }).click();
  expect(native.skill).toHaveBeenLastCalledWith(PROFILE, { kind: "delete", name: "job-search" });
});

afterEach(() => {
  vi.restoreAllMocks();
  delete document.documentElement.dataset.reduceMotion;
});

test.each([
  [false, false],
  [true, false],
  [false, true],
  [true, true],
])("skill reveal honors OS reduced motion %s and app reduced motion %s", async (os, app) => {
  const matchMedia = window.matchMedia.bind(window);
  vi.spyOn(window, "matchMedia").mockImplementation((query) =>
    query === "(prefers-reduced-motion: reduce)"
      ? { ...matchMedia(query), matches: os }
      : matchMedia(query),
  );
  document.documentElement.dataset.reduceMotion = String(app);
  const scroll = vi.spyOn(HTMLElement.prototype, "scrollIntoView").mockImplementation(() => {});
  const screen = await render(SkillsPage);
  await screen.getByRole("button", { name: /Weekly review/u }).click();
  expect(scroll).toHaveBeenCalledWith({
    block: "nearest",
    behavior: os || app ? "instant" : "smooth",
  });
});
