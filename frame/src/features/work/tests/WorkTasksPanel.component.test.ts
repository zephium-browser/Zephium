import "$styles/global.css";
import { expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page, userEvent } from "vitest/browser";
import { resourceTestServer } from "$shared/testing/resources/server";
import TasksPanelStage from "./TasksPanelStage.svelte";
import { loadTasks } from "$features/tasks";

const native = vi.hoisted(() => ({
  call: vi.fn(),
  open: vi.fn(),
  profile: "00000000000000000000000096",
}));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({ resourceCall: native.call, browserOpenUrl: native.open });
});
vi.mock("$domain/tabs", () => ({
  tabs: { profile: () => ({ id: native.profile }), activeTab: () => null },
}));
vi.mock("$domain/surface", () => ({ surface: { open: vi.fn() } }));

// Prepare the lazy body before timing task entry and view interaction.
await loadTasks();
const shots = "../../../../../target/work-shell";

test("the person's tasks over the canvas: pick a view, add one, complete it", async () => {
  await page.viewport(900, 760);
  native.call.mockImplementation(resourceTestServer(native.profile).call);
  const screen = await render(TasksPanelStage, { profile: native.profile });
  await screen.getByRole("tab", { name: "Inbox", exact: true }).click();
  await expect
    .element(screen.getByRole("tab", { name: "Inbox", exact: true }))
    .toHaveAttribute("aria-selected", "true");
  for (const title of ["Call the landlord", "Book the flight to SFO", "Send the deck to Ada"]) {
    const field = screen.getByRole("textbox", { name: "New task", exact: true });
    await field.click();
    await field.fill(title);
    await userEvent.keyboard("{Enter}");
    await expect.element(screen.getByText(title, { exact: true })).toBeVisible();
  }
  for (const theme of ["dark", "light"]) {
    document.documentElement.dataset.theme = theme;
    await new Promise((done) => setTimeout(done, 400));
    await page.screenshot({ path: `${shots}/tasks-${theme}.png` });
  }
  document.documentElement.dataset.theme = "dark";
  await screen.unmount();
});
