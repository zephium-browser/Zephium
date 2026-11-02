import { expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import "$styles/global.css";
import HidingBar from "../components/HidingBar.svelte";

vi.mock("$domain/blocker", () => ({
  hiding: {
    isActive: () => true,
    added: () => [],
    isSaving: () => false,
    undo: vi.fn(),
    finish: vi.fn(),
  },
}));
vi.mock("$domain/tabs", () => ({
  tabs: { activeId: () => "tab", activeTab: () => ({ url: "https://meet.example/" }) },
}));

test("the bar sits flush with the column's edges, which the window already insets", async () => {
  const screen = await render(HidingBar);
  const bar = screen.getByRole("status");
  await expect.element(bar).toBeVisible();
  const edges = bar.element().getBoundingClientRect();
  const column = screen.container.getBoundingClientRect();
  expect(edges.left).toBe(column.left);
  expect(edges.right).toBe(column.right);
});
