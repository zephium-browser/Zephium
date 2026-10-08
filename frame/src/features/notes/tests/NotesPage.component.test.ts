import { afterEach, expect, test, vi } from "vitest";
import { page, userEvent } from "vitest/browser";
import { render } from "vitest-browser-svelte";
import "$styles/global.css";
import primitives from "../../../styles/tokens/primitive.css?raw";
import tokens from "../../../styles/tokens.css?raw";
import browserCSS from "../../../styles/browser.css?raw";
import { notesTestServer } from "$shared/testing/notes/server";
import { emitNativeEvent } from "$shared/testing/native-events";
import PageHost from "./PageHost.svelte";

const native = vi.hoisted(() => ({
  call: vi.fn(),
  open: vi.fn(),
  profile: "01J9ZQ3V6Q4M8Y2K7T5R1N0C00",
}));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({ noteCall: native.call, browserOpenUrl: native.open });
});
const style = document.createElement("style");
style.textContent = primitives + tokens.replace("@theme static", ":root") + browserCSS;
document.head.append(style);
afterEach(() => {
  delete document.documentElement.dataset.theme;
});

const shot = (name: string) =>
  page.screenshot({ path: `../../../../../target/notes-qa/${name}.png` });

const DAY = 24 * 60 * 60 * 1000;

let profiles = 0;
async function setup() {
  profiles++;
  native.profile = `01J9ZQ3V6Q4M8Y2K7T5R1N0C${String(profiles).padStart(2, "0")}`;
  const server = notesTestServer(native.profile, (notes, reset, links) =>
    queueMicrotask(() =>
      emitNativeEvent("notesChanged", { profile: native.profile, notes, reset, links }),
    ),
  );
  native.call.mockImplementation(server.call);
  const now = Date.now();
  const at = (days: number) => String(now - days * DAY);
  server.seed(
    "# Reading list\n\n- [ ] *The Timeless Way of Building*\n- [x] Designing Data-Intensive Applications\n",
    { pinned: true, modified_at: at(20) },
  );
  server.seed(
    '# Trip to Lisbon\n\nBook the **train from Porto** on Friday, platform `3B`, then [[Packing list]].\n\n## Ideas\n\n- Pastéis de nata at Manteigaria\n- Sunset at the *Miradouro*\n\n> Travel light, stay longer.\n\n```ts\n// Leaves from Porto Campanhã\nconst train = { line: "CP 180", departs: "06:14", platform: 3 };\n```\n',
    { modified_at: at(0) },
  );
  server.seed("# Packing list\n\n- [ ] Passport\n- [ ] Adapter\n- [x] Camera\n", {
    modified_at: at(0.2),
  });
  server.seed("# Weekly review\n\nWhat moved, what stalled, what to drop.\n", {
    modified_at: at(1),
  });
  server.seed("# Browser launch plan\n\nShip notes as Markdown files people own.\n", {
    modified_at: at(4),
  });
  server.seed("# Recipes\n\nCaldo verde, bacalhau à Brás.\n", { modified_at: at(45) });
  const screen = await render(PageHost, { profile: native.profile });
  await expect.element(screen.getByText("Trip to Lisbon")).toBeVisible();
  return { screen, server };
}

test("the page lists notes by recency and edits one in a reading column", async () => {
  await page.viewport(1440, 900);
  document.documentElement.dataset.theme = "dark";
  const { screen, server } = await setup();
  await shot("page-list-dark");

  await screen.getByRole("option", { name: /^Trip to Lisbon/u }).click();
  const text = screen.getByRole("textbox", { name: "Note" });
  await expect.element(text).toBeVisible();
  await expect.element(text.getByText("Travel light, stay longer.")).toBeVisible();
  // Labelled code is coloured by the engine, with no elements added for it.
  const highlights = (CSS as unknown as { highlights: Map<string, Set<Range>> }).highlights;
  await expect.poll(() => highlights.get("note-code-string")?.size ?? 0).toBe(2);
  expect(highlights.get("note-code-keyword")?.size).toBe(1);
  expect(highlights.get("note-code-comment")?.size).toBe(1);
  expect(text.element().querySelector("pre code")!.children).toHaveLength(0);
  await new Promise((resolve) => setTimeout(resolve, 450));
  await shot("page-note-dark");

  // Typing is saved to the file as Markdown, a moment after it stops.
  const lisbon = [...server.notes.values()].find(
    (note) => note.summary.title === "Trip to Lisbon",
  )!;
  await text.getByText("Travel light, stay longer.").click();
  await userEvent.keyboard("{End} Always.");
  await expect
    .poll(() => server.notes.get(lisbon.summary.id)?.markdown, { timeout: 4000 })
    .toContain("> Travel light, stay longer. Always.");
  // Everything the edit did not touch is written back exactly as it was.
  expect(server.notes.get(lisbon.summary.id)?.markdown).toBe(
    lisbon.markdown.replace("stay longer.", "stay longer. Always."),
  );

  // Selecting text offers formatting.
  await userEvent.keyboard(
    "{Shift>}{ArrowLeft}{ArrowLeft}{ArrowLeft}{ArrowLeft}{ArrowLeft}{ArrowLeft}{ArrowLeft}{/Shift}",
  );
  await expect.element(screen.getByRole("toolbar", { name: "Formatting" })).toBeVisible();
  await new Promise((resolve) => setTimeout(resolve, 250));
  await shot("page-format-dark");
  await userEvent.keyboard("{Escape}");

  document.documentElement.dataset.theme = "light";
  await new Promise((resolve) => setTimeout(resolve, 300));
  await shot("page-note-light");
});

test("a new note starts at its title and becomes a file once it says something", async () => {
  await page.viewport(1440, 900);
  document.documentElement.dataset.theme = "dark";
  const { screen, server } = await setup();
  const before = server.notes.size;
  await screen.getByRole("button", { name: "New note", exact: true }).click();
  await expect.element(screen.getByRole("textbox", { name: "Note" })).toHaveFocus();
  await new Promise((resolve) => setTimeout(resolve, 450));
  await shot("page-new-dark");
  expect(server.notes.size).toBe(before);

  await userEvent.keyboard("Groceries{Enter}[[ ] Milk{Enter}Bread");
  await expect
    .poll(
      () => [...server.notes.values()].find((note) => note.summary.title === "Groceries")?.markdown,
      {
        timeout: 4000,
      },
    )
    .toBe("# Groceries\n\n- [ ] Milk\n- [ ] Bread\n");
  await expect.element(screen.getByRole("option", { name: /^Groceries/u })).toBeVisible();
  await new Promise((resolve) => setTimeout(resolve, 600));
  await shot("page-checklist-dark");
});

test("linking to another note suggests it as you type", async () => {
  await page.viewport(1440, 900);
  document.documentElement.dataset.theme = "dark";
  const { screen } = await setup();
  await screen.getByRole("option", { name: /^Weekly review/u }).click();
  const text = screen.getByRole("textbox", { name: "Note" });
  await text.getByText("What moved, what stalled, what to drop.").click();
  await userEvent.keyboard("{End}{Enter}See [[[[Trip");
  await expect.element(screen.getByRole("listbox", { name: "Link to a note" })).toBeVisible();
  await expect
    .element(
      screen
        .getByRole("listbox", { name: "Link to a note" })
        .getByRole("option", { name: /Trip to Lisbon/u }),
    )
    .toBeVisible();
  await shot("page-link-suggest-dark");
  await userEvent.keyboard("{Enter}");
  await expect.element(text.getByText("Trip to Lisbon", { exact: true })).toBeVisible();
});

test("search finds words inside notes and marks them", async () => {
  await page.viewport(1440, 900);
  document.documentElement.dataset.theme = "dark";
  const { screen } = await setup();
  await screen.getByRole("searchbox", { name: "Search notes" }).fill("porto");
  await expect.element(screen.getByRole("option", { name: /Trip to Lisbon/u })).toBeVisible();
  await expect.poll(() => screen.container.querySelectorAll("[role=option]").length).toBe(1);
  await shot("page-search-dark");
});

test("a narrow window shows the list or the note, with a way back", async () => {
  await page.viewport(820, 900);
  document.documentElement.dataset.theme = "dark";
  const { screen } = await setup();
  await screen.getByRole("option", { name: /^Packing list/u }).click();
  await expect.element(screen.getByRole("textbox", { name: "Note" })).toBeVisible();
  await shot("page-narrow-note-dark");
  await screen.getByRole("button", { name: "Notes", exact: true }).click();
  await expect.element(screen.getByRole("option", { name: /^Packing list/u })).toBeVisible();
});

test("typing that saves promptly does not flash a saving status", async () => {
  await page.viewport(1440, 900);
  const { screen, server } = await setup();
  await screen.getByRole("option", { name: /^Weekly review/u }).click();
  const text = screen.getByRole("textbox", { name: "Note" });
  const status = screen.container.querySelector(".stage-status")!;
  const seen: string[] = [];
  const watch = new MutationObserver(() => seen.push(status.textContent ?? ""));
  watch.observe(status, { childList: true, characterData: true, subtree: true });
  await text.getByText("What moved, what stalled, what to drop.").click();
  await userEvent.keyboard("{End} Then rest.");
  const id = [...server.notes.values()].find((note) => note.summary.title === "Weekly review")!
    .summary.id;
  await expect
    .poll(() => server.notes.get(id)?.markdown, { timeout: 4000 })
    .toContain("Then rest.");
  watch.disconnect();
  expect(seen.filter((entry) => entry.includes("Saving"))).toEqual([]);
});

test("hiding the window saves the open note at once", async () => {
  await page.viewport(1440, 900);
  const { screen, server } = await setup();
  await screen.getByRole("option", { name: /^Weekly review/u }).click();
  const text = screen.getByRole("textbox", { name: "Note" });
  await text.getByText("What moved, what stalled, what to drop.").click();
  await userEvent.keyboard("{End} Hidden.");
  const id = [...server.notes.values()].find((note) => note.summary.title === "Weekly review")!
    .summary.id;
  Object.defineProperty(document, "visibilityState", { configurable: true, get: () => "hidden" });
  try {
    document.dispatchEvent(new Event("visibilitychange"));
    await expect.poll(() => server.notes.get(id)?.markdown, { timeout: 500 }).toContain("Hidden.");
  } finally {
    delete (document as { visibilityState?: unknown }).visibilityState;
  }
});
