import { expect, test, vi } from "vitest";
import { page, userEvent } from "vitest/browser";
import { render } from "vitest-browser-svelte";
import { resourceTestServer } from "$shared/testing/resources/server";
import { emitNativeEvent } from "$shared/testing/native-events";
import type { ResourceRecord_Serialize } from "$shared/ipc/bindings";
import TaskHost from "./TaskHost.svelte";
import { dayKey } from "../lib/task-sections";
import { addDays } from "../lib/task-calendar";

const native = vi.hoisted(() => ({ call: vi.fn(), open: vi.fn() }));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({ resourceCall: native.call, browserOpenUrl: native.open });
});

const today = dayKey(new Date());
let seq = 0;

function server(profile: string) {
  const made = resourceTestServer(profile);
  native.call.mockImplementation(made.call);
  return made;
}

/** A task already in the store, so a test can start from a populated list. */
function seed(
  records: Map<string, ResourceRecord_Serialize>,
  title: string,
  extra: { due_date?: string | null; status?: "open" | "active" | "blocked" | "done" } = {},
) {
  const id = String(++seq).padStart(26, "0");
  records.set(id, {
    id,
    revision: "1",
    created_at: "100",
    updated_at: "100",
    trashed: false,
    draft: {
      title,
      pinned: false,
      related: [],
      content: {
        kind: "task",
        details: {},
        description: "",
        completed: extra.status === "done",
        due_date: extra.due_date ?? null,
        status: extra.status ?? "open",
        assignee: "user",
        origin: "user",
        context: null,
        sort_key: null,
        work: null,
      },
    },
  });
  return id;
}

const tasks = (records: Map<string, ResourceRecord_Serialize>) =>
  [...records.values()].map((record) => record.draft.title);

test("captures one task after another without leaving an empty record behind", async () => {
  await page.viewport(900, 800);
  const profile = "00000000000000000000000011";
  const { records } = server(profile);
  const screen = await render(TaskHost, { profile, scope: "all" });

  const field = screen.getByRole("textbox", { name: "New task", exact: true });
  await field.fill("Review the vendor contract");
  await field.click();
  await userEvent.keyboard("{Enter}");
  await expect.poll(() => tasks(records)).toEqual(["Review the vendor contract"]);

  // The field stays open and empty, so the next one costs nothing.
  await expect.element(field).toHaveValue("");
  await expect.element(field).toHaveFocus();
  await field.fill("Book the flight");
  await userEvent.keyboard("{Enter}");
  await expect
    .poll(() => tasks(records))
    .toEqual(["Review the vendor contract", "Book the flight"]);

  // Pressing Enter on nothing writes nothing: no "New task" placeholder exists.
  await userEvent.keyboard("{Enter}");
  await expect.poll(() => records.size).toBe(2);
});

test("completing a task settles as done and keeps its place while it does", async () => {
  await page.viewport(900, 800);
  const profile = "00000000000000000000000012";
  const { records } = server(profile);
  const id = seed(records, "Renew the certificate", { due_date: today });
  const screen = await render(TaskHost, { profile });

  const box = screen.getByRole("checkbox", { name: /Renew the certificate/u });
  await expect.element(box).toBeVisible();
  await box.click();

  await expect
    .poll(() => {
      const content = records.get(id)!.draft.content;
      return content.kind === "task" ? [content.status, content.completed] : null;
    })
    .toEqual(["done", true]);
  // Still in the list it was completed in, rather than vanishing under the pointer.
  await expect.element(screen.getByText("Renew the certificate")).toBeVisible();
});

test("schedules a task from its own row without leaving the list", async () => {
  await page.viewport(900, 800);
  const profile = "00000000000000000000000013";
  const { records } = server(profile);
  const id = seed(records, "Compare both vendors", { due_date: today });
  const screen = await render(TaskHost, { profile, scope: "all" });

  await screen.getByRole("button", { name: "Schedule", exact: true }).click();
  await screen.getByRole("button", { name: /Tomorrow/u }).click();

  await expect
    .poll(() => {
      const content = records.get(id)!.draft.content;
      return content.kind === "task" ? content.due_date : null;
    })
    .toBe(addDays(today, 1));
  // The list is still the list; the row moved to the day it now belongs to.
  await expect.element(screen.getByText("Compare both vendors")).toBeVisible();
  await expect
    .poll(() => {
      const nodes = [...screen.container.querySelectorAll(".task-heading, .task-label")];
      const at = nodes.findIndex((node) => node.textContent?.includes("Compare both vendors"));
      return at > 0 ? nodes[at - 1]?.getAttribute("data-key") : null;
    })
    .toBe("tomorrow");
});

test("groups by when work is due and names the sections", async () => {
  await page.viewport(900, 800);
  const profile = "00000000000000000000000014";
  const { records } = server(profile);
  seed(records, "Overdue invoice", { due_date: addDays(today, -3) });
  seed(records, "Standup notes", { due_date: today });
  seed(records, "Someday idea");
  const screen = await render(TaskHost, { profile, scope: "all" });

  await expect.element(screen.getByRole("heading", { name: /Overdue/u })).toBeVisible();
  await expect.element(screen.getByRole("heading", { name: /Today/u })).toBeVisible();
  await expect.element(screen.getByRole("heading", { name: /Anytime/u })).toBeVisible();

  const headings = await screen.container.querySelectorAll(".task-heading");
  expect([...headings].map((node) => node.textContent?.replace(/\d+$/u, ""))).toEqual([
    "Overdue",
    "Today",
    "Anytime",
  ]);
});

test("moves through the list and completes a task without the pointer", async () => {
  await page.viewport(900, 800);
  const profile = "00000000000000000000000015";
  const { records } = server(profile);
  seed(records, "First task", { due_date: today });
  const second = seed(records, "Second task", { due_date: today });
  const screen = await render(TaskHost, { profile });

  await expect.element(screen.getByText("Second task")).toBeVisible();
  await screen.getByRole("textbox", { name: "New task", exact: true }).click();
  await userEvent.keyboard("{Escape}");
  await userEvent.keyboard("{ArrowDown}");
  await userEvent.keyboard(" ");

  await expect
    .poll(() => {
      const content = records.get(second)!.draft.content;
      return content.kind === "task" ? content.status : null;
    })
    .toBe("done");
});

test("a task captured while looking at Today is due today", async () => {
  await page.viewport(900, 800);
  const profile = "00000000000000000000000016";
  const { records } = server(profile);
  const screen = await render(TaskHost, { profile, scope: "today" });

  const field = screen.getByRole("textbox", { name: "New task", exact: true });
  await field.fill("Call the supplier");
  await field.click();
  await userEvent.keyboard("{Enter}");

  await expect
    .poll(() => {
      const content = [...records.values()][0]?.draft.content;
      return content?.kind === "task" ? content.due_date : undefined;
    })
    .toBe(today);
  // And it is visible where it was typed, rather than filed somewhere unseen.
  await expect.element(screen.getByText("Call the supplier")).toBeVisible();
});

test("deleting a task from the keyboard can be undone", async () => {
  await page.viewport(900, 800);
  const profile = "00000000000000000000000017";
  const { records } = server(profile);
  const id = seed(records, "Draft the summary", { due_date: today });
  const screen = await render(TaskHost, { profile });

  await expect.element(screen.getByText("Draft the summary", { exact: true })).toBeVisible();
  await screen.getByRole("textbox", { name: "New task", exact: true }).click();
  await userEvent.keyboard("{Escape}");
  await userEvent.keyboard("{ArrowDown}");
  await userEvent.keyboard("{Control>}{Backspace}{/Control}");
  await expect.poll(() => records.get(id)!.trashed).toBe(true);

  await userEvent.keyboard("{Control>}z{/Control}");
  await expect.poll(() => records.get(id)!.trashed).toBe(false);
  await expect.element(screen.getByText("Draft the summary", { exact: true })).toBeVisible();
});

test("the rail draws the same rows in a narrower shape", async () => {
  await page.viewport(900, 800);
  const profile = "00000000000000000000000018";
  const { records } = server(profile);
  seed(records, "Compile the findings", { due_date: today });
  const screen = await render(TaskHost, { profile, density: "rail" });

  await expect.element(screen.getByText("Compile the findings")).toBeVisible();
  const row = screen.container.querySelector<HTMLElement>(".task")!;
  expect(Math.round(row.getBoundingClientRect().height)).toBe(38);
  // Still a task, not a label: the box works exactly as it does in the panel.
  await screen.getByRole("checkbox", { name: /Compile the findings/u }).click();
  await expect
    .poll(() => {
      const content = [...records.values()][0]?.draft.content;
      return content?.kind === "task" ? content.status : null;
    })
    .toBe("done");
});

test("a change made elsewhere settles into the list instead of raising a conflict", async () => {
  await page.viewport(900, 800);
  const profile = "00000000000000000000000019";
  const { records } = server(profile);
  const id = seed(records, "Confirm the booking", { due_date: today });
  const screen = await render(TaskHost, { profile });
  await expect.element(screen.getByText("Confirm the booking")).toBeVisible();

  // What an agent working this task looks like from here: the record moves on,
  // and native says so.
  const record = records.get(id)!;
  const content = record.draft.content;
  if (content.kind !== "task") throw new Error("seeded a task");
  records.set(id, {
    ...record,
    revision: "2",
    draft: {
      ...record.draft,
      title: "Confirm the booking with the vendor",
      content: { ...content, status: "blocked", assignee: "agent" },
    },
  });
  emitNativeEvent("resourceChanged", { profile, kind: "task", id, revision: "2" });

  await expect.element(screen.getByText("Confirm the booking with the vendor")).toBeVisible();
  // It reads as a task that needs a person, not as a dialog asking about drafts.
  await expect.element(screen.getByRole("checkbox", { name: /Blocked/u })).toBeVisible();
  expect(screen.container.querySelector(".save-status")).toBeNull();
});

test("the capture field reads a date out of the line before committing it", async () => {
  await page.viewport(900, 800);
  const profile = "00000000000000000000000020";
  const { records } = server(profile);
  const screen = await render(TaskHost, { profile, scope: "all" });

  const field = screen.getByRole("textbox", { name: "New task", exact: true });
  await field.fill("Call Anna tomorrow at 3pm");
  // What it understood is shown before Enter, not discovered afterwards.
  await expect
    .element(screen.getByRole("button", { name: "Keep “tomorrow at 3pm” as words" }))
    .toBeVisible();

  await field.click();
  await userEvent.keyboard("{Enter}");
  await expect
    .poll(() => {
      const record = [...records.values()][0];
      const content = record?.draft.content;
      return content?.kind === "task"
        ? [record!.draft.title, content.due_date, content.due_time]
        : null;
    })
    .toEqual(["Call Anna", addDays(today, 1), "15:00"]);
});

test("turning down the reading keeps the words", async () => {
  await page.viewport(900, 800);
  const profile = "00000000000000000000000021";
  const { records } = server(profile);
  const screen = await render(TaskHost, { profile, scope: "all" });

  const field = screen.getByRole("textbox", { name: "New task", exact: true });
  await field.fill("Ship the Friday build");
  // "Friday" is mid-line, so there is nothing to turn down in the first place.
  expect(screen.container.querySelector(".capture-refuse")).toBeNull();

  await field.fill("Plan the retro friday");
  await screen.getByRole("button", { name: "Keep “friday” as words" }).click();
  await field.click();
  await userEvent.keyboard("{Enter}");
  await expect
    .poll(() => {
      const record = [...records.values()][0];
      const content = record?.draft.content;
      return content?.kind === "task" ? [record!.draft.title, content.due_date] : null;
    })
    .toEqual(["Plan the retro friday", null]);
});

test("a search finds a match that the current scope would have hidden", async () => {
  await page.viewport(900, 800);
  const profile = "00000000000000000000000022";
  const { records } = server(profile);
  seed(records, "Renew the vendor contract", { due_date: addDays(today, 30) });
  seed(records, "Standup notes", { due_date: today });
  // Today is the scope, and the match is a month away.
  const screen = await render(TaskHost, { profile, scope: "today", query: "vendor" });

  await expect.element(screen.getByText("Renew the vendor contract")).toBeVisible();
  // And the row says why it matched.
  await expect.poll(() => screen.container.querySelector("mark")?.textContent).toBe("vendor");
});

test("a selection acts on every task in it at once", async () => {
  await page.viewport(900, 800);
  const profile = "00000000000000000000000023";
  const { records } = server(profile);
  seed(records, "First", { due_date: today });
  seed(records, "Second", { due_date: today });
  seed(records, "Third", { due_date: today });
  const screen = await render(TaskHost, { profile });
  await expect.element(screen.getByText("Third")).toBeVisible();

  await screen.getByRole("textbox", { name: "New task", exact: true }).click();
  await userEvent.keyboard("{Escape}");
  await userEvent.keyboard("{ArrowDown}");
  await userEvent.keyboard("{Control>}a{/Control}");
  await expect.element(screen.getByRole("toolbar", { name: /3 selected/u })).toBeVisible();

  await userEvent.keyboard(" ");
  await expect
    .poll(() =>
      [...records.values()].every((record) => {
        const content = record.draft.content;
        return content.kind === "task" && content.status === "done";
      }),
    )
    .toBe(true);
});

test("a long list draws a window of rows, not all of them", async () => {
  await page.viewport(900, 800);
  const profile = "00000000000000000000000024";
  const { records } = server(profile);
  for (let index = 0; index < 400; index += 1)
    seed(records, `Task ${String(index).padStart(3, "0")}`, { due_date: today });
  const screen = await render(TaskHost, { profile });

  await expect.poll(() => screen.container.querySelectorAll(".task").length).toBeGreaterThan(0);
  const drawn = screen.container.querySelectorAll(".task").length;
  expect(drawn).toBeLessThan(80);

  // Scrolling the list moves the window rather than revealing pre-rendered rows.
  const scroller = screen.container.querySelector<HTMLElement>(".task-scroller")!;
  const firstBefore = screen.container.querySelector(".task-label")?.textContent;
  scroller.scrollTop = 4000;
  scroller.dispatchEvent(new Event("scroll"));
  await expect
    .poll(() => screen.container.querySelector(".task-label")?.textContent)
    .not.toBe(firstBefore);
  expect(screen.container.querySelectorAll(".task").length).toBeLessThan(80);
});

test("a task's notes are read when it is opened, not left blank", async () => {
  await page.viewport(900, 800);
  const profile = "00000000000000000000000025";
  const { records } = server(profile);
  const id = seed(records, "Renew the lease", { due_date: today });
  const stored = records.get(id)!;
  const body = stored.draft.content;
  if (body.kind !== "task") throw new Error("seeded a task");
  records.set(id, {
    ...stored,
    draft: { ...stored.draft, content: { ...body, description: "Ask about the break clause" } },
  });

  const screen = await render(TaskHost, { profile });
  await screen.getByText("Renew the lease").click();

  // A listing carries no body, so opening the row has to go and read it.
  await expect
    .element(screen.getByRole("textbox", { name: "Description", exact: true }))
    .toHaveValue("Ask about the break clause");
});

test("a Today row does not repeat the day its section already names", async () => {
  await page.viewport(900, 800);
  const profile = "00000000000000000000000026";
  const { records } = server(profile);
  seed(records, "Standup", { due_date: today });
  seed(records, "Quarterly review", { due_date: addDays(today, 40) });
  const screen = await render(TaskHost, { profile, scope: "all" });

  await expect.element(screen.getByText("Standup")).toBeVisible();
  const chipFor = (title: string) =>
    [...screen.container.querySelectorAll<HTMLElement>(".task")]
      .find((row) => row.textContent?.includes(title))
      ?.querySelector(".task-due")
      ?.textContent?.trim() ?? "";

  expect(chipFor("Standup")).toBe("");
  // A month out, the row has to say which day.
  expect(chipFor("Quarterly review")).not.toBe("");
});

test("typing in a task says nothing about saving unless a save is slow", async () => {
  await page.viewport(900, 800);
  const profile = "00000000000000000000000027";
  const made = server(profile);
  const id = seed(made.records, "Call mom", { due_date: today });
  const screen = await render(TaskHost, { profile });
  await screen.getByText("Call mom").click();
  const title = screen.getByRole("textbox", { name: "Rename", exact: true });
  const status = screen.container.querySelector(".detail-save")!;

  const seen: string[] = [];
  const watch = new MutationObserver(() => seen.push(status.textContent ?? ""));
  watch.observe(status, { childList: true, characterData: true, subtree: true });
  await title.fill("Call mom today");
  await expect
    .poll(() => made.records.get(id)!.draft.title, { timeout: 4000 })
    .toBe("Call mom today");
  expect(seen.filter((text) => text.includes("Saving"))).toEqual([]);
  watch.disconnect();

  let release!: () => void;
  const gate = new Promise<void>((resolve) => (release = resolve));
  native.call.mockImplementation(async (owner, call) => {
    if (call.kind === "mutate") await gate;
    return made.call(owner, call);
  });
  await title.fill("Call mom tonight");
  await expect.element(screen.getByText("Saving…")).toBeVisible();
  release();
  await expect.poll(() => status.textContent).toBe("");
});

test("the capture field keeps focus and keys while a task is being created", async () => {
  await page.viewport(900, 800);
  const profile = "00000000000000000000000028";
  const made = server(profile);
  let release!: () => void;
  let fail = false;
  const gate = new Promise<void>((resolve) => (release = resolve));
  native.call.mockImplementation(async (owner, call) => {
    if (call.kind === "mutate") {
      await gate;
      if (fail) return { profile, response: { kind: "error", error: "unavailable" } };
    }
    return made.call(owner, call);
  });
  const screen = await render(TaskHost, { profile, scope: "all" });
  const field = screen.getByRole("textbox", { name: "New task", exact: true });
  await field.click();
  await userEvent.keyboard("Water the plants{Enter}Feed");
  await expect.element(field).toHaveValue("Feed");
  await expect.element(field).toBeEnabled();
  await expect.element(field).toHaveFocus();
  release();
  await expect.poll(() => tasks(made.records)).toEqual(["Water the plants"]);
  await expect.element(field).toHaveValue("Feed");

  // A capture that fails gives its words back to an empty field.
  fail = true;
  await userEvent.keyboard(" the cat{Enter}");
  await expect.element(field).toHaveValue("Feed the cat");
  expect(tasks(made.records)).toEqual(["Water the plants"]);
});

test("hiding the window saves a title being typed at once", async () => {
  await page.viewport(900, 800);
  const profile = "00000000000000000000000029";
  const made = server(profile);
  const id = seed(made.records, "Plan", { due_date: today });
  const screen = await render(TaskHost, { profile });
  await screen.getByText("Plan").click();
  const title = screen.getByRole("textbox", { name: "Rename", exact: true });
  await title.fill("Plan the trip");
  Object.defineProperty(document, "visibilityState", { configurable: true, get: () => "hidden" });
  try {
    document.dispatchEvent(new Event("visibilitychange"));
    await expect
      .poll(() => made.records.get(id)!.draft.title, { timeout: 500 })
      .toBe("Plan the trip");
  } finally {
    delete (document as { visibilityState?: unknown }).visibilityState;
  }
});

test("the list behind an open task catches up once it is shown again", async () => {
  await page.viewport(900, 800);
  const profile = "00000000000000000000000031";
  const made = server(profile);
  seed(made.records, "Draft memo", { due_date: today });
  const screen = await render(TaskHost, { profile });
  await screen.getByText("Draft memo").click();
  const title = screen.getByRole("textbox", { name: "Rename", exact: true });
  await title.fill("Draft the memo");
  // Hidden, the list is not regrouped for every key.
  expect(screen.container.querySelector(".task-label")?.textContent).toBe("Draft memo");
  await screen.getByRole("button", { name: "Back to tasks" }).click();
  await expect.element(screen.getByText("Draft the memo")).toBeVisible();
});

test("a title renamed from its row is saved as soon as it is entered", async () => {
  await page.viewport(900, 800);
  const profile = "00000000000000000000000032";
  const made = server(profile);
  const id = seed(made.records, "Old name", { due_date: today });
  const screen = await render(TaskHost, { profile });
  await screen.getByText("Old name").hover();
  await screen.getByRole("button", { name: "More actions" }).click();
  await screen.getByRole("menuitem", { name: "Rename" }).click();
  const field = screen.container.querySelector<HTMLTextAreaElement>(".task-rename")!;
  await expect.poll(() => document.activeElement === field).toBe(true);
  await userEvent.keyboard("New name{Enter}");
  await expect.poll(() => made.records.get(id)!.draft.title, { timeout: 500 }).toBe("New name");
});
