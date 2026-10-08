import { afterEach, expect, test, vi } from "vitest";
import { page, userEvent } from "vitest/browser";
import { render } from "vitest-browser-svelte";
import "$styles/global.css";
import primitives from "../../../styles/tokens/primitive.css?raw";
import tokens from "../../../styles/tokens.css?raw";
import browserCSS from "../../../styles/browser.css?raw";
import { notesTestServer } from "$shared/testing/notes/server";
import { emitNativeEvent } from "$shared/testing/native-events";
import ResourceHost from "./ResourceHost.svelte";

const native = vi.hoisted(() => ({ call: vi.fn(), open: vi.fn() }));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({ noteCall: native.call, browserOpenUrl: native.open });
});
vi.mock("$domain/surface", () => ({ surface: { open: vi.fn() } }));
const style = document.createElement("style");
style.textContent = primitives + tokens.replace("@theme static", ":root") + browserCSS;
document.head.append(style);
afterEach(() => {
  delete document.documentElement.dataset.theme;
});

const shot = (name: string) =>
  page.screenshot({ path: `../../../../../target/notes-qa/${name}.png` });

let profiles = 0;
function server() {
  profiles++;
  const profile = `01J9ZQ3V6Q4M8Y2K7T5R1N0D${String(profiles).padStart(2, "0")}`;
  const notes = notesTestServer(profile, (changed, reset, links) =>
    queueMicrotask(() =>
      emitNativeEvent("notesChanged", { profile, notes: changed, reset, links }),
    ),
  );
  native.call.mockImplementation(notes.call);
  const now = Date.now();
  notes.seed("# Reading list\n\n- [ ] The Timeless Way of Building\n", {
    pinned: true,
    modified_at: String(now - 9 * 86_400_000),
  });
  notes.seed(
    "# Trip to Lisbon\n\nBook the **train from Porto** on Friday.\n\n- [ ] Pack the adapter\n- [x] Buy tickets\n",
    { modified_at: String(now) },
  );
  notes.seed("# Weekly review\n\nWhat moved, what stalled, what to drop.\n", {
    modified_at: String(now - 86_400_000),
  });
  return { profile, notes };
}

test("notes read, open and come back in the sidebar panel", async () => {
  await page.viewport(1000, 800);
  document.documentElement.dataset.theme = "dark";
  const { profile, notes } = server();
  const screen = await render(ResourceHost, { profile, tool: "notes" });
  await expect.element(screen.getByRole("option", { name: /^Trip to Lisbon/u })).toBeVisible();
  await shot("panel-sidebar-list-dark");
  // Long previews never widen the panel past its host.
  const tool = screen.container.querySelector<HTMLElement>(".shared-tool")!;
  expect(tool.scrollWidth).toBeLessThanOrEqual(tool.clientWidth + 1);
  expect(tool.offsetWidth).toBeLessThanOrEqual(384 - 56);

  await screen.getByRole("option", { name: /^Trip to Lisbon/u }).click();
  const text = screen.getByRole("textbox", { name: "Note" });
  await expect.element(text).toBeVisible();
  // Search gives way to the note; the header becomes the way back.
  expect(screen.container.querySelector("input[type=search]")).toBeNull();
  await new Promise((resolve) => setTimeout(resolve, 450));
  await shot("panel-sidebar-note-dark");

  await text.getByText("Pack the adapter").click();
  await userEvent.keyboard("{End} and cables");
  const id = [...notes.notes.values()].find((note) => note.summary.title === "Trip to Lisbon")!
    .summary.id;
  await expect
    .poll(() => notes.notes.get(id)?.markdown, { timeout: 4000 })
    .toContain("- [ ] Pack the adapter and cables\n- [x] Buy tickets");

  // The title is the way back.
  await screen.getByRole("button", { name: "Notes", exact: true }).click();
  await expect.element(screen.getByRole("searchbox", { name: "Search notes" })).toBeVisible();
  // The note just edited leads its day.
  const titles = [...screen.container.querySelectorAll(".note-row-title")].map(
    (row) => row.textContent,
  );
  expect(titles.slice(0, 2)).toEqual(["Reading list", "Trip to Lisbon"]);
});

test("a new note in the panel is saved once it has a title", async () => {
  await page.viewport(1000, 800);
  document.documentElement.dataset.theme = "dark";
  const { profile, notes } = server();
  const screen = await render(ResourceHost, {
    profile,
    tool: "notes",
  });
  await screen.getByRole("button", { name: "New note", exact: true }).click();
  await expect.element(screen.getByRole("textbox", { name: "Note" })).toHaveFocus();
  await userEvent.keyboard("Standup{Enter}Ship the notes redesign");
  await expect
    .poll(
      () => [...notes.notes.values()].find((note) => note.summary.title === "Standup")?.markdown,
      {
        timeout: 4000,
      },
    )
    .toBe("# Standup\n\nShip the notes redesign\n");
  await new Promise((resolve) => setTimeout(resolve, 300));
  await shot("panel-sidebar-new-dark");
});

test("an empty panel invites the first note", async () => {
  await page.viewport(1000, 800);
  document.documentElement.dataset.theme = "dark";
  profiles++;
  const profile = `01J9ZQ3V6Q4M8Y2K7T5R1N0E${String(profiles).padStart(2, "0")}`;
  native.call.mockImplementation(notesTestServer(profile).call);
  const screen = await render(ResourceHost, {
    profile,
    tool: "notes",
  });
  await expect.element(screen.getByText("No notes yet")).toBeVisible();
  await shot("panel-sidebar-empty-dark");
});

test("formatting stays inside the sidebar, clear of the page beside it", async () => {
  await page.viewport(1000, 800);
  document.documentElement.dataset.theme = "dark";
  const { profile } = server();
  const screen = await render(ResourceHost, {
    profile,
    tool: "notes",
  });
  await screen.getByRole("option", { name: /^Trip to Lisbon/u }).click();
  const text = screen.getByRole("textbox", { name: "Note" });
  await expect.element(text).toBeVisible();
  const area = screen.container.querySelector<HTMLElement>("[data-note-bounds]")!;
  const inside = (element: Element) => {
    const box = element.getBoundingClientRect();
    const bounds = area.getBoundingClientRect();
    expect(box.left).toBeGreaterThanOrEqual(bounds.left);
    expect(box.right).toBeLessThanOrEqual(bounds.right);
    expect(box.top).toBeGreaterThanOrEqual(bounds.top);
    expect(box.bottom).toBeLessThanOrEqual(bounds.bottom);
  };

  // The title's first line has no room above it, so the bar opens below.
  await text.getByText("Book the").click();
  await userEvent.keyboard("{End}{Shift>}{ArrowLeft}{ArrowLeft}{ArrowLeft}{ArrowLeft}{/Shift}");
  const bar = screen.getByRole("toolbar", { name: "Formatting" });
  await expect.element(bar).toBeVisible();
  await new Promise((resolve) => setTimeout(resolve, 250));
  inside(bar.element());
  await screen.getByRole("button", { name: /^Formatting: /u }).click();
  const styles = screen.getByRole("menu", { name: "Formatting" });
  await expect.element(styles).toBeVisible();
  await new Promise((resolve) => setTimeout(resolve, 250));
  inside(styles.element());
  expect(styles.element().getBoundingClientRect().top).toBeGreaterThan(
    bar.element().getBoundingClientRect().bottom,
  );
  await shot("panel-sidebar-format-dark");
  await userEvent.keyboard("{Escape}");

  // Typing `/` on a new line offers every style, inside the panel too.
  await text.getByText("Buy tickets").click();
  await userEvent.keyboard("{End}{Enter}{Enter}/");
  const commands = screen.getByRole("listbox", { name: "Formatting" });
  await expect.element(commands).toBeVisible();
  await new Promise((resolve) => setTimeout(resolve, 250));
  inside(commands.element());
  await shot("panel-sidebar-commands-dark");
  await userEvent.keyboard("head");
  await userEvent.keyboard("{Enter}Next week");
  await expect.element(text.getByRole("heading", { name: "Next week" })).toBeVisible();
});
