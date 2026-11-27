import { expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import "$styles/global.css";
import * as notices from "$session/notice.svelte";
import PrivateReturn from "../components/PrivateReturn.svelte";
import SidebarNotice from "../components/SidebarNotice.svelte";

vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({});
});
vi.mock("$domain/profiles", () => ({
  profiles: { retain: () => {}, release: () => {}, privateAside: () => true, privateTabs: () => 2 },
}));

// The window already holds the chrome off its own edge, so a card that adds a
// margin of its own looks inset twice.
function flush(card: Element, column: Element) {
  const edges = card.getBoundingClientRect();
  const box = column.getBoundingClientRect();
  expect(edges.left).toBe(box.left);
  expect(edges.right).toBe(box.right);
}

test("a notice sits flush with the column's edges", async () => {
  const screen = await render(SidebarNotice);
  notices.show("Link copied");
  await expect.element(screen.getByRole("status")).toBeVisible();
  flush(screen.getByRole("status").element(), screen.container);
});

test("the way back to the private window sits flush with the column's edges", async () => {
  const screen = await render(PrivateReturn);
  const button = screen.getByRole("button", { name: /Back to private window/u });
  await expect.element(button).toBeVisible();
  flush(button.element(), screen.container);
});
