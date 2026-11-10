import { afterEach, expect, test, vi } from "vitest";
import { page, userEvent } from "vitest/browser";
import { render } from "vitest-browser-svelte";
import { resourceTestServer } from "$shared/testing/resources/server";
import primitives from "../../../styles/tokens/primitive.css?raw";
import tokens from "../../../styles/tokens.css?raw";
import browserCSS from "../../../styles/browser.css?raw";
import ResourceHost from "./ResourceHost.svelte";
const native = vi.hoisted(() => ({ call: vi.fn(), open: vi.fn() }));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({ resourceCall: native.call, browserOpenUrl: native.open });
});
const style = document.createElement("style");
style.textContent = primitives + tokens.replace("@theme static", ":root") + browserCSS;
afterEach(() => style.remove());

const LONG = "A long title that must remain inside its own resource surface";

/** Nothing the tool draws may reach past the tool, at either host width. */
function fitted(container: HTMLElement, root: string, parts: string) {
  const panel = container.querySelector<HTMLElement>(root);
  if (!panel) return false;
  const bounds = panel.parentElement!.getBoundingClientRect();
  const box = panel.getBoundingClientRect();
  const contentFits = [...panel.querySelectorAll<HTMLElement>(parts)].every((element) => {
    if (!element.getClientRects().length) return true;
    const child = element.getBoundingClientRect();
    return (
      child.left >= box.left - 1 &&
      child.right <= box.right + 1 &&
      element.scrollWidth <= element.clientWidth + 1
    );
  });
  return (
    contentFits &&
    box.width > 300 &&
    box.left >= bounds.left &&
    box.right <= bounds.right + 1 &&
    panel.scrollWidth <= panel.clientWidth + 1
  );
}

test("tasks fit the sidebar ToolSlot while capturing and completing in place", async () => {
  await page.viewport(1000, 800);
  document.head.append(style);
  const profile = "00000000000000000000000004";
  const server = resourceTestServer(profile);
  native.call.mockImplementation(server.call);
  const screen = await render(ResourceHost, { profile, tool: "tasks" });
  await expect.element(screen.getByRole("region", { name: "Tasks", exact: true })).toBeVisible();
  const fits = () => fitted(screen.container, ".shared-tool", ".task-scroller, .capture, .task");
  await expect.poll(fits).toBe(true);
  await page.screenshot({ path: "../../../../../target/tasks-qa/tool-sidebar.png" });

  const field = screen.getByRole("textbox", { name: "New task", exact: true });
  await field.fill(LONG);
  await field.click();
  await userEvent.keyboard("{Enter}");
  await expect.poll(() => [...server.records.values()][0]?.draft.title).toBe(LONG);
  // A title far wider than the panel still leaves the row inside the tool.
  await expect.poll(fits).toBe(true);

  await screen.getByRole("checkbox", { name: new RegExp(LONG, "u") }).click();
  await expect
    .poll(() => {
      const content = [...server.records.values()][0]?.draft.content;
      return content?.kind === "task" ? content.status : null;
    })
    .toBe("done");
  // Completing never navigated: the capture field is still the one in view.
  await expect.element(field).toBeVisible();
});

test("the tasks panel keeps search in view without taking focus from the page", async () => {
  await page.viewport(1000, 800);
  document.head.append(style);
  const profile = "00000000000000000000000005";
  const server = resourceTestServer(profile);
  native.call.mockImplementation(server.call);
  const screen = await render(ResourceHost, {
    profile,
    tool: "tasks",
  });
  await expect.element(screen.getByRole("region", { name: "Tasks", exact: true })).toBeVisible();

  // The scope is the title; More holds the tool's own actions, the page and
  // closing.
  await expect.element(screen.getByRole("button", { name: /Show/u })).toBeVisible();
  const header = screen.container.querySelector(".shared-tool-header")!;
  expect(
    [...header.querySelectorAll('button:not([role="menuitem"])')].map((button) => button.ariaLabel),
  ).toEqual(["Show", "More actions"]);
  expect(header.querySelector(".shared-tool-icon")).toBeNull();
  await screen.getByRole("button", { name: "More actions", exact: true }).click();
  await expect.element(screen.getByRole("menuitem", { name: "Trash" })).toBeVisible();
  await expect.element(screen.getByRole("menuitem", { name: "Open as a page" })).toBeVisible();
  await expect.element(screen.getByRole("menuitem", { name: "Close tool" })).toBeVisible();
  await userEvent.keyboard("{Escape}");

  const field = screen.getByRole("searchbox", { name: "Search tasks", exact: true });
  await expect.element(field).toBeVisible();
  expect(document.activeElement?.getAttribute("aria-label")).not.toBe("Search tasks");

  // Escape clears a query and leaves the field where it is.
  await field.fill("vendor");
  await userEvent.keyboard("{Escape}");
  await expect.element(field).toHaveValue("");
  await expect.element(field).toBeVisible();
});

test("the scope menu changes what the panel is showing", async () => {
  await page.viewport(1000, 800);
  document.head.append(style);
  const profile = "00000000000000000000000006";
  const server = resourceTestServer(profile);
  native.call.mockImplementation(server.call);
  const screen = await render(ResourceHost, {
    profile,
    tool: "tasks",
  });

  const scope = screen.getByRole("button", { name: /Show/u });
  await expect.element(scope).toHaveTextContent("Today");
  await scope.click();
  await screen.getByRole("menuitemcheckbox", { name: "All tasks", exact: true }).click();
  await expect.element(screen.getByRole("button", { name: /Show/u })).toHaveTextContent("All");
});
