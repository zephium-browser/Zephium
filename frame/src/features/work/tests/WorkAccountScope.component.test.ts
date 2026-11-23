import "$styles/global.css";
import { expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page, userEvent } from "vitest/browser";
import { WorkEnvironmentSession } from "$domain/work-environment";
import type { WorkEnvironmentSnapshot } from "$shared/ipc/bindings";
import { tabFixture } from "$shared/testing/fixtures";
import WorkEnvironmentWorkspace from "../components/WorkEnvironmentWorkspace.svelte";
const native = vi.hoisted(() => ({ call: vi.fn() }));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockComposerBindings } = await import("./composer-fixtures");
  return mockComposerBindings({ faviconProbe: async () => true, workCall: native.call });
});

const profile = "00000000000000000000000001";
const tabElement = "00000000000000000000000004";

async function mountCanvas() {
  await page.viewport(1200, 800);
  const { workSession } = await import("$domain/work");
  const { projection } = await import("./environment-fixtures");
  const snapshot: WorkEnvironmentSnapshot = {
    version: 1,
    id: "00000000000000000000000003",
    profile,
    space: "00000000000000000000000002",
    title: "Sprint",
    lifecycle: "active",
    revision: "1",
    elements: [{ id: tabElement, area: null, reference: { kind: "browser", tab: "tab-1" } }],
    areas: [],
    view: {
      revision: "1",
      x: 0,
      y: 0,
      zoom_milli: 1000,
      placements: [{ element: tabElement, x: 80, y: 80, width: 320, height: 200 }],
    },
  };
  native.call.mockResolvedValue({
    version: 1,
    profile,
    reply: { kind: "environment", reply: { kind: "snapshot", snapshot } },
  });
  const environment = new WorkEnvironmentSession(profile, snapshot.space);
  environment.snapshot = snapshot;
  environment.selected = snapshot.id;
  const objective = workSession(profile)!;
  vi.spyOn(objective, "start").mockResolvedValue();
  vi.spyOn(objective, "open").mockResolvedValue(true);
  const create = vi.spyOn(objective, "create").mockImplementation(async (text) => {
    objective.selected = "objective";
    objective.projection = {
      ...projection,
      executions: [],
      work: { ...projection.work, profile, status: "draft", objective: text },
    };
    return true;
  });
  const operation = vi.spyOn(objective.operations, "begin").mockImplementation(async (input) => ({
    id: "operation",
    input,
    state: { kind: "pending", work: "objective" },
  }));
  const run = vi.spyOn(objective, "run").mockResolvedValue(true);
  for (const spy of [create, operation, run]) spy.mockClear();
  vi.spyOn(environment, "edit").mockResolvedValue(true);
  const screen = await render(WorkEnvironmentWorkspace, {
    session: environment,
    tabs: [
      tabFixture({ id: "tab-1", title: "Sprint 42", url: "https://app.notion.com/p/Sprint-42" }),
    ],
    spaceName: "Personal",
    aiEnabled: true,
    onopen: vi.fn(),
    onnewtab: vi.fn(),
  });
  const root = screen.container.querySelector(".environment") as HTMLElement;
  root.style.height = "720px";
  root.style.width = "1100px";
  const composer = screen.getByRole("textbox", { name: "What do you want to do?", exact: true });
  return { screen, environment, objective, snapshot, create, operation, run, composer };
}

test("all open tabs is consent for one request, shown in the composer", async () => {
  const { screen, environment, snapshot, run, composer } = await mountCanvas();
  await screen
    .getByRole("toolbar", { name: "Work tools" })
    .getByRole("button", { name: "Add to canvas", exact: true })
    .click();
  const all = screen.getByRole("checkbox", { name: /All open tabs/ });
  await expect
    .element(
      screen.getByText("Titles and addresses of the tabs in this window, not their contents", {
        exact: true,
      }),
    )
    .toBeVisible();
  await all.click();
  await expect.element(all).toBeChecked();
  await userEvent.keyboard("{Escape}");
  // Context is shown when the compact composer is engaged.
  await composer.click();
  await expect.element(screen.getByText("Using your open tabs", { exact: true })).toBeVisible();
  await composer.fill("What am I researching?");
  await screen.getByRole("button", { name: "Send", exact: true }).click();
  await expect.poll(() => run.mock.calls.length).toBe(1);
  expect(run.mock.calls[0]![0]).toEqual({ environment: snapshot.id, items: [], tabs: true });
  // For this request only: the next one starts without it.
  await expect
    .element(screen.getByText("Using your open tabs", { exact: true }))
    .not.toBeInTheDocument();
  await screen.unmount();
  environment.dispose();
});
