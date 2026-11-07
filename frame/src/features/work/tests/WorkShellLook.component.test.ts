import "$styles/global.css";
import { afterEach, expect, test, vi } from "vitest";
import { cleanup, render } from "vitest-browser-svelte";
import { page, userEvent } from "vitest/browser";
import { WorkEnvironmentSession } from "$domain/work-environment";
import WorkEnvironmentWorkspace from "../components/WorkEnvironmentWorkspace.svelte";
import { workSession } from "$domain/work";
import { projection, snapshot } from "./environment-fixtures";
// Prepare the lazy surfaces before each visual case starts its interaction budget.
import "../components/WorkCanvas.svelte";
import "../components/bar/ModelPicker.svelte";
import "../components/AgentLine.svelte";

vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  const { models } = await import("$shared/testing/work-models");
  return mockBindings({
    workModels: vi.fn(async (profile: string) => ({
      ...models({ keys: { anthropic: "valid" }, lead: "anthropic/claude-opus-5-5" }),
      profile,
    })),
    faviconProbe: async () => true,
    runCommand: vi.fn(async () => ({ accepted: true, operation_id: null })),
    workCall: vi.fn(async (profile: string) => ({
      version: 1,
      profile,
      reply: { kind: "error" as const, error: "not_found" as const },
    })),
  });
});

const shots = "../../../../../target/work-shell";
let activeEnvironment: WorkEnvironmentSession | undefined;
afterEach(() => {
  cleanup();
  if (activeEnvironment) {
    activeEnvironment.composer = "";
    activeEnvironment.dispose();
    activeEnvironment = undefined;
  }
  workSession(snapshot.profile).back();
  vi.restoreAllMocks();
  document.documentElement.dataset.theme = "dark";
});

async function mount() {
  await page.viewport(1440, 900);
  const environment = new WorkEnvironmentSession(snapshot.profile, snapshot.space);
  activeEnvironment = environment;
  environment.composer = "";
  environment.snapshot = { ...structuredClone(snapshot), title: "YC trip from Warsaw" };
  environment.selected = snapshot.id;
  const hour = 3_600_000;
  const now = Date.now();
  environment.works = [
    ["YC trip from Warsaw", snapshot.id, "Plan my YC batch trip from Warsaw, three months", 0],
    ["AI SaaS architecture and budget", "a", "Design a modern AI SaaS architecture", 5],
    ["New work", "b", "Compare SQLite, DuckDB and RocksDB for an embedded analytics cache", 30],
    ["New work", "c", "", 40],
    ["Compiler learning plan", "e", "Find the best free university material for compilers", 80],
    ["Old launch plan", "d", "Plan the launch", 400],
  ].map(([title, id, request, ago]) => ({
    id: id as string,
    space: snapshot.space,
    title: title as string,
    lifecycle: id === "d" ? ("archived" as const) : ("active" as const),
    revision: "1",
    name: null,
    requests: request ? [request as string] : [],
    touched_ms: String(now - (ago as number) * hour),
    empty: !request,
  }));
  const objective = workSession(snapshot.profile)!;
  objective.back();
  vi.spyOn(objective, "start").mockResolvedValue();
  vi.spyOn(objective, "open").mockImplementation(async () => {
    objective.projection = structuredClone(projection);
    return true;
  });
  const screen = await render(WorkEnvironmentWorkspace, {
    session: environment,
    tabs: [],
    spaceName: "Personal",
    aiEnabled: true,
    onopen: vi.fn(),
    onnewtab: vi.fn(),
  });
  const root = screen.container.querySelector(".environment") as HTMLElement;
  root.style.height = "884px";
  root.style.width = "1376px";
  return { screen, objective };
}

for (const theme of ["dark", "light"]) {
  for (const state of ["rest", "hint", "typing", "works", "note", "island"] as const) {
    test(`the Work chrome at ${state}, ${theme}`, async () => {
      document.documentElement.dataset.theme = theme;
      const { screen, objective } = await mount();
      if (state === "hint") {
        await screen.getByRole("button", { name: "Hand", exact: true }).hover();
        await new Promise((done) => setTimeout(done, 900));
      } else if (state === "typing") {
        const field = screen.getByRole("textbox", { name: "What do you want to do?" });
        await field.click();
        await userEvent.keyboard("Plan my trip to the next YC batch from Warsaw");
        await expect
          .poll(() => screen.container.querySelector<HTMLButtonElement>(".model-trigger")?.disabled)
          .toBe(false);
      } else if (state === "works") {
        await screen.getByRole("button", { name: "Works" }).click();
        await expect.element(screen.getByRole("searchbox", { name: "Search works" })).toBeVisible();
        expect(screen.container.querySelector('[aria-label="Account"]')).toBeNull();
      } else if (state === "note") {
        await screen.getByRole("button", { name: "Blocks", exact: true }).click();
        await page.getByRole("button", { name: /^Note/u }).click();
        await expect.element(screen.getByRole("searchbox", { name: "Search notes" })).toBeVisible();
      } else if (state === "island") {
        const live = structuredClone(projection);
        const execution = live.executions[0]!;
        execution.status = "running";
        execution.authorization = "user_directed_agent";
        execution.attempts = [{ id: "attempt", node: "node", status: "running", usage: null }];
        execution.steps = [
          {
            id: "read-1",
            turn: 1,
            kind: { kind: "read", url: "https://www.airbnb.com/s/San-Francisco" },
            status: "running",
          },
        ];
        objective.selected = "objective";
        objective.projection = live;
        await expect.poll(() => screen.container.querySelector(".island")).not.toBeNull();
      }
      await new Promise((done) => setTimeout(done, 450));
      await page.screenshot({ path: `${shots}/${state}-${theme}.png` });
    });
  }
}
