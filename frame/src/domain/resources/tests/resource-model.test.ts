import { expect, test } from "vitest";
import type { ResourceSummary } from "$shared/ipc/bindings";
import { settleInto } from "../resource-model";

const summary = (id: string, title = id): ResourceSummary => ({
  id,
  revision: "1",
  title,
  pinned: false,
  updated_at: "1",
  completed: false,
  due_date: null,
  due_time: null,
  status: "open",
  assignee: "user",
  origin: "user",
  context: null,
  sort_key: null,
  work: null,
});

test("a task saved into a full listing stays, and the last row loaded gives way", () => {
  const full = Array.from({ length: 1000 }, (_, index) => summary(String(index)));
  const settled = settleInto(full, summary("new"));
  expect(settled).toHaveLength(1000);
  expect(settled[0]!.id).toBe("new");
  expect(settled.some((item) => item.id === "999")).toBe(false);
});

test("a task already listed is replaced where it is", () => {
  const items = [summary("a"), summary("b"), summary("c")];
  const settled = settleInto(items, summary("b", "Renamed"));
  expect(settled.map((item) => item.title)).toEqual(["a", "Renamed", "c"]);
});
