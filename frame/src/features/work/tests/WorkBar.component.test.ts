import "$styles/global.css";
import { expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page } from "vitest/browser";
import { WorkEnvironmentSession } from "$domain/work-environment";
import type { WorkEnvironmentSnapshot, WorkRuntimeProjection } from "$shared/ipc/bindings";
import { tabFixture } from "$shared/testing/fixtures";
import WorkEnvironmentWorkspace from "../components/WorkEnvironmentWorkspace.svelte";
import { projection } from "./environment-fixtures";
// Compile the lazy surfaces before the interaction readiness deadline starts.
import "../components/WorkCanvas.svelte";
import "../components/AgentLine.svelte";

const native = vi.hoisted(() => ({ call: vi.fn() }));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({ faviconProbe: async () => true, workCall: native.call });
});

const profile = "bar-profile";
const shots = "../../../../../target/work-shell";
const settle = () => new Promise((done) => setTimeout(done, 450));

function running(): WorkRuntimeProjection {
  const state = structuredClone(projection);
  state.work = { ...state.work, profile };
  const execution = state.executions[0]!;
  execution.status = "running";
  execution.authorization = "user_directed_agent";
  execution.attempts = [{ id: "attempt", node: "node", status: "running", usage: null }];
  execution.spec.nodes[0]!.capability = {
    kind: "agent",
    grant: {
      provider: "open_ai",
      model: "gpt-5.6-luna",
      max_turns: 10,
      max_steps: 32,
      browse_hops: 4,
    },
  };
  execution.steps = [
    {
      id: "read-1",
      turn: 1,
      kind: { kind: "read", url: "https://www.airbnb.com/s/San-Francisco" },
      status: "running",
    },
  ];
  return state;
}

async function shoot(root: HTMLElement, name: string) {
  for (const theme of ["dark", "light"]) {
    document.documentElement.dataset.theme = theme;
    await settle();
    await page.elementLocator(root).screenshot({ path: `${shots}/${name}-${theme}.png` });
  }
  document.documentElement.dataset.theme = "dark";
}

test("the bar is tools and a field at rest, the field alone in use, the agent's line in a run", async () => {
  await page.viewport(1200, 800);
  const { workSession } = await import("$domain/work");
  const tab = "00000000000000000000000004";
  const snapshot: WorkEnvironmentSnapshot = {
    version: 1,
    id: "00000000000000000000000003",
    profile,
    space: "space",
    title: "YC trip",
    lifecycle: "active",
    revision: "1",
    elements: [{ id: tab, area: null, reference: { kind: "browser", tab: "tab-1" } }],
    areas: [],
    view: {
      revision: "1",
      x: 0,
      y: 0,
      zoom_milli: 1000,
      placements: [{ element: tab, x: 120, y: 24, width: 320, height: 200 }],
    },
  };
  native.call.mockResolvedValue({
    version: 1,
    profile,
    reply: { kind: "error", error: "not_found" },
  });
  const environment = new WorkEnvironmentSession(profile, snapshot.space);
  environment.snapshot = snapshot;
  environment.selected = snapshot.id;
  const objective = workSession(profile)!;
  vi.spyOn(objective, "start").mockResolvedValue();
  vi.spyOn(objective, "open").mockResolvedValue(true);
  vi.spyOn(environment, "edit").mockResolvedValue(true);
  const screen = await render(WorkEnvironmentWorkspace, {
    session: environment,
    tabs: [tabFixture({ id: "tab-1", title: "Stays in the Mission", url: "https://airbnb.com/" })],
    spaceName: "Personal",
    aiEnabled: true,
    onopen: vi.fn(),
    onnewtab: vi.fn(),
  });
  const root = screen.container.querySelector<HTMLElement>(".environment")!;
  root.style.height = "720px";
  root.style.width = "1100px";
  const bar = () => screen.container.querySelector<HTMLElement>(".work-bar")!;

  // At rest: the canvas tools, then the field; nothing on the right.
  const tools = screen.getByRole("toolbar", { name: "Work tools" });
  for (const name of ["Blocks", "Add to canvas"])
    await expect.element(tools.getByRole("button", { name, exact: true })).toBeVisible();
  expect(bar().dataset.mode).toBe("rest");
  expect(screen.container.querySelector("[data-zephium-work-chrome]")).toBeNull();
  await expect
    .poll(() => screen.container.querySelector("[data-work-zoom-slot] [role='group']"))
    .not.toBeNull();
  await shoot(root, "bar-rest");

  // The work's name heads the canvas as the way to every other work.
  await expect.element(screen.getByRole("button", { name: "Works" })).toHaveTextContent("YC trip");

  // In use: the tools step aside and the field opens its own controls.
  const field = screen.getByRole("textbox", { name: "What do you want to do?", exact: true });
  await field.click();
  await field.fill("Plan the trip");
  await expect.poll(() => bar().dataset.mode).toBe("focus");
  expect(bar().querySelector(".tools")?.getAttribute("aria-hidden")).toBe("true");
  await expect.element(screen.getByRole("button", { name: "Add to canvas" })).toBeVisible();
  await expect.element(screen.getByRole("button", { name: "Send", exact: true })).toBeEnabled();
  await shoot(root, "bar-focus");
  await field.fill("");
  (document.activeElement as HTMLElement | null)?.blur();
  await expect.poll(() => bar().dataset.mode).toBe("rest");

  // A run: its line stands at the canvas's top; the bar stays a thin field that adds to it.
  objective.selected = "objective";
  objective.projection = running();
  environment.snapshot = {
    ...snapshot,
    elements: [
      ...snapshot.elements,
      {
        id: "objective-card",
        area: null,
        reference: { kind: "objective", objective: "objective" },
      },
    ],
  };
  await expect.poll(() => screen.container.querySelector(".island .agent-line")).not.toBeNull();
  expect(bar().dataset.mode).toBe("rest");
  expect(bar().querySelector(".agent-line")).toBeNull();
  await expect.element(screen.getByRole("button", { name: "Stop" })).toBeVisible();
  await expect
    .element(screen.getByRole("textbox", { name: "Add to this…", exact: true }))
    .toBeVisible();
  expect(screen.container.querySelector(".works-trigger .live")).not.toBeNull();
  await shoot(root, "bar-running");

  // Finished: the island keeps the run's last words; the bar is unchanged.
  const done = running();
  done.executions[0]!.status = "completed";
  done.executions[0]!.steps![0]!.status = "succeeded";
  objective.projection = done;
  await expect.poll(() => bar().dataset.mode).toBe("rest");
  await expect.element(screen.getByRole("toolbar", { name: "Work tools" })).toBeVisible();
  expect(screen.container.querySelector(".island .agent-line.settled")).not.toBeNull();
  await shoot(root, "bar-settled");

  await screen.unmount();
  environment.dispose();
  objective.dispose();
});
