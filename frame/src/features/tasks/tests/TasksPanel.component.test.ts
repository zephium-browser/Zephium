import { afterEach, expect, test, vi } from "vitest";
import { page, userEvent } from "vitest/browser";
import { render } from "vitest-browser-svelte";
import "$styles/global.css";
import primitives from "../../../styles/tokens/primitive.css?raw";
import tokens from "../../../styles/tokens.css?raw";
import { resourceTestServer } from "$shared/testing/resources/server";
import { taskSession } from "$domain/resources";
import { dayKey } from "../lib/task-sections";
import { addDays } from "../lib/task-calendar";
import TaskHost from "./TaskHost.svelte";

const native = vi.hoisted(() => ({ call: vi.fn(), open: vi.fn() }));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({ resourceCall: native.call, browserOpenUrl: native.open });
});
const style = document.createElement("style");
style.textContent = primitives + tokens.replace("@theme static", ":root");
document.head.append(style);
afterEach(() => {
  delete document.documentElement.dataset.theme;
});

const today = dayKey(new Date());

async function seed(profile: string) {
  const server = resourceTestServer(profile);
  native.call.mockImplementation(server.call);
  const session = taskSession(profile, "sidebar");
  const work = await session.saveList("Work");
  await session.create({
    title: "Send the investor update before the board meeting on Thursday",
    dueDate: today,
    dueTime: "15:00",
    deadline: addDays(today, 2),
    duration: 45,
    priority: "high",
    list: work!.id,
    context: { url: "https://docs.example.com/update", title: "Investor update draft" },
  });
  await session.create({ title: "Review the storage changes", dueDate: today, priority: "low" });
  await session.create({ title: "Renew the domain", deadline: today, priority: "medium" });
  return server;
}

test("a narrow row keeps every property on one quiet line under its title", async () => {
  await page.viewport(900, 800);
  document.documentElement.dataset.theme = "dark";
  const profile = "00000000000000000000000081";
  await seed(profile);
  const screen = await render(TaskHost, { profile, scope: "today" });

  await expect.element(screen.getByText("Renew the domain")).toBeVisible();
  // Owed today by its deadline alone, so Today holds it without a planned day.
  await expect.element(screen.getByLabelText(/^Deadline /u).first()).toBeVisible();
  await expect.element(screen.getByText("45m")).toBeVisible();
  await expect.element(screen.getByText("Work", { exact: true })).toBeVisible();
  await expect.element(screen.getByText("docs.example.com")).toBeVisible();
  // Priority is a monochrome signal on the metadata line, never a coloured ring.
  await expect.element(screen.getByRole("img", { name: "Priority: High" })).toBeVisible();
  expect(screen.container.querySelector(".task-check[data-priority]")).toBeNull();
  await page.screenshot({ path: "../../../../../target/tasks-qa/panel-dark.png" });
});

test("completing a task offers to take it back", async () => {
  await page.viewport(900, 800);
  const profile = "00000000000000000000000082";
  const server = await seed(profile);
  const screen = await render(TaskHost, { profile, scope: "today" });

  await screen.getByRole("checkbox", { name: /Review the storage changes/u }).click();
  await expect.element(screen.getByRole("group", { name: "Completed" })).toBeVisible();
  await page.screenshot({ path: "../../../../../target/tasks-qa/panel-notice.png" });
  await screen.getByRole("button", { name: "Undo", exact: true }).click();
  await expect
    .poll(
      () =>
        [...server.records.values()].find((record) => record.draft.title.startsWith("Review"))
          ?.draft.content,
    )
    .toMatchObject({ status: "open", completed: false });
});

test("choosing from a composer picker keeps the composer open", async () => {
  await page.viewport(900, 800);
  document.documentElement.dataset.theme = "dark";
  const profile = "00000000000000000000000083";
  const server = await seed(profile);
  const screen = await render(TaskHost, { profile, scope: "today" });

  const field = screen.getByRole("textbox", { name: "New task", exact: true });
  await field.click();
  await screen.getByRole("button", { name: "List", exact: true }).click();
  await screen.getByRole("menuitemcheckbox", { name: "Work", exact: true }).click();
  await expect
    .element(screen.getByRole("button", { name: "List", exact: true }))
    .toHaveTextContent("Work");
  await field.fill("Draft the agenda for a long planning session with the whole team next week");
  await page.screenshot({ path: "../../../../../target/tasks-qa/panel-composer.png" });
  await field.click();
  await userEvent.keyboard("{Enter}");
  await expect
    .poll(() => {
      const record = [...server.records.values()].find((entry) =>
        entry.draft.title.startsWith("Draft the agenda"),
      );
      return record?.draft.content.kind === "task" ? record.draft.content.details.list : undefined;
    })
    .toBe([...server.lists.values()][0]!.id);
});

test("rename from the row menu edits in place and wraps a long title", async () => {
  await page.viewport(900, 800);
  const profile = "00000000000000000000000084";
  const server = await seed(profile);
  const screen = await render(TaskHost, { profile, scope: "today" });

  await screen.getByText("Renew the domain").hover();
  await screen.getByRole("button", { name: "More actions" }).last().click();
  await screen.getByRole("menuitem", { name: "Rename", exact: true }).click();
  const rename = screen.getByRole("textbox", { name: "Rename", exact: true });
  await expect.poll(() => document.activeElement?.getAttribute("aria-label")).toBe("Rename");
  await rename.fill("Renew the domain and move its records to the new provider before it expires");
  await userEvent.keyboard("{Enter}");
  await expect
    .poll(() =>
      [...server.records.values()].some((entry) => entry.draft.title.endsWith("before it expires")),
    )
    .toBe(true);
});

test("a typed time shows what it will become and is kept on Enter", async () => {
  await page.viewport(900, 800);
  const profile = "00000000000000000000000085";
  const server = await seed(profile);
  const screen = await render(TaskHost, { profile, scope: "today" });

  await screen.getByText("Renew the domain").hover();
  await screen.getByRole("button", { name: "Schedule" }).last().click();
  const time = screen.getByRole("textbox", { name: "Time" });
  await time.click();
  await userEvent.keyboard("4:30pm");
  await expect.element(screen.getByText("4:30 PM")).toBeVisible();
  await page.screenshot({ path: "../../../../../target/tasks-qa/due-menu.png" });
  await userEvent.keyboard("{Enter}");
  await expect
    .poll(() => {
      const record = [...server.records.values()].find(
        (entry) => entry.draft.title === "Renew the domain",
      );
      return record?.draft.content.kind === "task" ? record.draft.content.due_time : undefined;
    })
    .toBe("16:30");
});

test("a task is dragged into place, and the keyboard can move it too", async () => {
  await page.viewport(900, 800);
  const profile = "00000000000000000000000086";
  const server = resourceTestServer(profile);
  native.call.mockImplementation(server.call);
  const session = taskSession(profile, "sidebar");
  for (const title of ["Alpha", "Bravo", "Charlie"]) await session.create({ title });
  const screen = await render(TaskHost, { profile, scope: "all" });
  await expect.element(screen.getByText("Charlie")).toBeVisible();

  const order = () =>
    [...screen.container.querySelectorAll<HTMLElement>(".task-label")].map(
      (node) => node.textContent,
    );
  const initial = order();
  const slot = (title: string) =>
    [...screen.container.querySelectorAll<HTMLElement>(".task-slot")].find((node) =>
      node.textContent?.includes(title),
    )!;
  const from = slot(initial[2]!);
  const to = slot(initial[0]!);
  const start = from.getBoundingClientRect();
  const end = to.getBoundingClientRect();
  const at = (x: number, y: number, type: string) =>
    from.dispatchEvent(
      new PointerEvent(type, {
        bubbles: true,
        cancelable: true,
        pointerId: 1,
        button: 0,
        clientX: x,
        clientY: y,
      }),
    );
  at(start.left + 60, start.top + 10, "pointerdown");
  at(start.left + 60, start.top - 10, "pointermove");
  at(end.left + 60, end.top + 4, "pointermove");
  await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
  at(end.left + 60, end.top + 4, "pointerup");
  await expect.poll(order).toEqual([initial[2], initial[0], initial[1]]);

  // Option-Command-Down takes the selected task one place later.
  screen.container.querySelector<HTMLElement>(".task-scroller")!.focus();
  await userEvent.keyboard("{ArrowDown}");
  await userEvent.keyboard("{Alt>}{Meta>}{ArrowDown}{/Meta}{/Alt}");
  await expect.poll(order).toEqual([initial[0], initial[2], initial[1]]);
  await expect
    .poll(() =>
      [...server.records.values()]
        .map((record) => ({
          title: record.draft.title,
          key: record.draft.content.kind === "task" ? record.draft.content.sort_key : null,
        }))
        .sort((a, b) => (a.key ?? "~").localeCompare(b.key ?? "~"))
        .map((row) => row.title),
    )
    .toEqual([initial[0], initial[2], initial[1]]);
});
