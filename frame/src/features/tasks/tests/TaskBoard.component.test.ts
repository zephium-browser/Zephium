import { expect, test, vi } from "vitest";
import { page } from "vitest/browser";
import { render } from "vitest-browser-svelte";
import { resourceTestServer } from "$shared/testing/resources/server";
import type { ResourceRecord_Serialize } from "$shared/ipc/bindings";
import BoardHost from "./BoardHost.svelte";
import { orderValue } from "../lib/task-order";

const native = vi.hoisted(() => ({ call: vi.fn(), open: vi.fn() }));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({ resourceCall: native.call, browserOpenUrl: native.open });
});

let seq = 0;

function seed(
  records: Map<string, ResourceRecord_Serialize>,
  title: string,
  status: "open" | "active" | "blocked" | "done",
  sortKey: string | null = null,
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
        completed: status === "done",
        due_date: null,
        due_time: null,
        status,
        assignee: "user",
        origin: "user",
        context: null,
        sort_key: sortKey,
        work: null,
      },
    },
  });
  return id;
}

const stateOf = (records: Map<string, ResourceRecord_Serialize>, id: string) => {
  const content = records.get(id)!.draft.content;
  return content.kind === "task" ? content : null;
};

/** The board's own gesture: press, cross the threshold, release over a target. */
async function dragTo(from: Element, to: Element) {
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
  at(start.left + 20, start.top + 10, "pointerdown");
  at(start.left + 40, start.top + 30, "pointermove");
  at(end.left + end.width / 2, end.top + Math.min(40, end.height / 2), "pointermove");
  // Two frames: the gesture throttles its position to one per frame.
  await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
  at(end.left + end.width / 2, end.top + Math.min(40, end.height / 2), "pointerup");
}

test("dragging a card to another column moves the task into that state", async () => {
  await page.viewport(1100, 800);
  const profile = "00000000000000000000000031";
  const server = resourceTestServer(profile);
  native.call.mockImplementation(server.call);
  const id = seed(server.records, "Book the venue", "open");
  const screen = await render(BoardHost, { profile });

  await expect.element(screen.getByText("Book the venue")).toBeVisible();
  const card = screen.container.querySelector(`[data-task-card="${id}"]`)!;
  const target = screen.container.querySelector('[data-task-column="active"]')!;
  await dragTo(card, target);

  await expect.poll(() => stateOf(server.records, id)?.status).toBe("active");
  // Landing in a column also gives the card a place in it.
  await expect.poll(() => orderValue(stateOf(server.records, id)?.sort_key ?? null)).not.toBeNull();
});

test("a card dropped into Done is completed, not merely filed", async () => {
  await page.viewport(1100, 800);
  const profile = "00000000000000000000000032";
  const server = resourceTestServer(profile);
  native.call.mockImplementation(server.call);
  const id = seed(server.records, "Send the contract", "open");
  const screen = await render(BoardHost, { profile });

  await expect.element(screen.getByText("Send the contract")).toBeVisible();
  await dragTo(
    screen.container.querySelector(`[data-task-card="${id}"]`)!,
    screen.container.querySelector('[data-task-column="done"]')!,
  );

  await expect
    .poll(() => {
      const task = stateOf(server.records, id);
      return task && [task.status, task.completed];
    })
    .toEqual(["done", true]);
});

test("a press that does not travel stays a click, not a drag", async () => {
  await page.viewport(1100, 800);
  const profile = "00000000000000000000000033";
  const server = resourceTestServer(profile);
  native.call.mockImplementation(server.call);
  const id = seed(server.records, "Review the brief", "open");
  const screen = await render(BoardHost, { profile });

  await expect.element(screen.getByText("Review the brief")).toBeVisible();
  const card = screen.container.querySelector(`[data-task-card="${id}"]`)!;
  const box = card.getBoundingClientRect();
  for (const [type, dx] of [
    ["pointerdown", 0],
    ["pointermove", 2],
    ["pointerup", 2],
  ] as const)
    card.dispatchEvent(
      new PointerEvent(type, {
        bubbles: true,
        cancelable: true,
        pointerId: 1,
        button: 0,
        clientX: box.left + 20 + dx,
        clientY: box.top + 10,
      }),
    );

  // Nothing moved, so nothing was written.
  await expect.poll(() => stateOf(server.records, id)?.status).toBe("open");
});

test("columns order their cards by the place a drag gave them", async () => {
  await page.viewport(1100, 800);
  const profile = "00000000000000000000000034";
  const server = resourceTestServer(profile);
  native.call.mockImplementation(server.call);
  seed(server.records, "Second", "open", "000000002000");
  seed(server.records, "First", "open", "000000001000");
  const screen = await render(BoardHost, { profile });

  await expect.element(screen.getByText("First")).toBeVisible();
  await expect
    .poll(() =>
      [...screen.container.querySelectorAll('[data-task-column="open"] .board-card-title')].map(
        (node) => node.textContent?.trim(),
      ),
    )
    .toEqual(["First", "Second"]);
});

test("a card can change status through its accessible actions", async () => {
  await page.viewport(1100, 800);
  const profile = "00000000000000000000000036";
  const server = resourceTestServer(profile);
  native.call.mockImplementation(server.call);
  const id = seed(server.records, "Review the delivery", "open");
  const screen = await render(BoardHost, { profile });
  await expect.element(screen.getByText("Review the delivery")).toBeVisible();
  await screen.getByRole("button", { name: "More actions", exact: true }).click();
  await screen.getByRole("menuitemcheckbox", { name: "In progress", exact: true }).click();
  await expect.poll(() => stateOf(server.records, id)?.status).toBe("active");
});
