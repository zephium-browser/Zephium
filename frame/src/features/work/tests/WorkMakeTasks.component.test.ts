import "$styles/global.css";
import { expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page } from "vitest/browser";
import { TaskSession } from "$domain/resources";
import WorkCanvas from "../components/WorkCanvas.svelte";
// Prepare the lazy renderer before timing task interaction on the canvas.
import "../components/objects/Plan.svelte";
import { environmentBoards, environmentItems } from "../lib/project-environment";
import { environmentStages } from "../lib/project-environment-board";
import { artifactView } from "../lib/project-work";
import { resultPlan } from "../lib/plan-steps";
import { stepPlan, WorkTasks, workTasksKey } from "../lib/work-tasks";
import { planScene } from "./environment-fixtures";

const native = vi.hoisted(() => ({ resource: vi.fn() }));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({ resourceCall: native.resource });
});

test("a plan's steps become the person's tasks on request, and a task done in Tasks strikes its step", async () => {
  const { resourceTestServer } = await import("$shared/testing/resources/server");
  const { emitNativeEvent } = await import("$shared/testing/native-events");
  await page.viewport(1400, 900);
  const profile = "profile";
  const server = resourceTestServer(profile);
  native.resource.mockImplementation(server.call);
  const { scene, objectives } = planScene();
  // The plan as a checklist: its steps are the block's rows.
  const run = objectives.get("objective")!.executions[0]!;
  run.artifacts[0]!.data = {
    kind: "checklist",
    items: [
      "Confirm the batch and interview dates",
      "Check your ESTA eligibility",
      "Compare Airbnb flats for total cost",
      "Plan the SFO-to-stay route by BART",
    ].map((text) => ({ text, completed: false })),
  };
  const stages = environmentStages(scene, objectives);
  const view = artifactView(run.artifacts[0]!, run);
  const items = [
    ...environmentItems(scene, [], [], objectives),
    ...environmentBoards(stages, (id) => (id === "plan-card" ? view : undefined)),
  ];
  const plan = stepPlan("plan-card", "objective", view, resultPlan(view.content));
  const session = new TaskSession(profile);
  await session.start();
  const open = vi.fn();
  const tasks = new WorkTasks(profile, session, () => new Map([["plan-card", plan]]), open);
  const screen = await render(WorkCanvas, {
    props: {
      items,
      links: [],
      authoritative: new Set(["objective-card", "plan-card"]),
      initialView: {
        positions: stages[0]!.targets,
        viewport: { x: 16, y: 16, zoom: 0.8 },
      },
      oninspect: vi.fn(),
    },
    context: new Map([[workTasksKey, tasks]]),
  });
  screen.container.style.width = "1400px";
  screen.container.style.height = "900px";
  const cards = () => [...screen.container.querySelectorAll<HTMLElement>(".plan .step")];
  await expect.poll(() => cards().length).toBe(4);
  // Nothing is made until the person asks.
  expect(server.records.size).toBe(0);
  expect(tasks.tasks("plan-card").filter(Boolean)).toHaveLength(0);

  await screen.getByRole("button", { name: "Make tasks", exact: true }).click();
  await expect.poll(() => tasks.tasks("plan-card").filter(Boolean).length).toBe(4);
  const made = [...server.records.values()]
    .map((record) => ({ title: record.draft.title, task: record.draft.content }))
    .toSorted((a, b) =>
      a.task.kind === "task" && b.task.kind === "task"
        ? (a.task.sort_key ?? "").localeCompare(b.task.sort_key ?? "")
        : 0,
    );
  expect(made.map((entry) => entry.title)).toEqual(plan.steps.map((step) => step.title));
  for (const { task } of made) {
    expect(task).toMatchObject({
      kind: "task",
      origin: "agent",
      assignee: "user",
      status: "open",
      work: "objective",
    });
  }
  // Asked twice for the same lane, the action is spent: the steps are the tasks now and
  // nothing announces it.
  await expect
    .element(screen.getByRole("button", { name: "Make tasks", exact: true }))
    .not.toBeInTheDocument();
  expect(await tasks.make("plan-card")).toBe(false);
  expect(server.records.size).toBe(4);

  // Completed in Tasks: another session writes it, this one hears the change.
  const first = tasks.tasks("plan-card")[0]!;
  const elsewhere = new TaskSession(profile);
  await elsewhere.start();
  await elsewhere.load(first.id);
  expect(await elsewhere.setStatus(first.id, "done")).toBe(true);
  emitNativeEvent("resourceChanged", {
    profile,
    kind: "task",
    id: first.id,
    revision: server.records.get(first.id)!.revision,
  });
  await expect.poll(() => cards()[0]!.classList.contains("done")).toBe(true);
  expect(cards()[1]!.classList.contains("done")).toBe(false);
  cards()[0]!.querySelector<HTMLElement>(".check")!.click();
  expect(open).toHaveBeenCalledExactlyOnceWith(first.id);
  await screen.unmount();
  session.stop();
  elsewhere.stop();
});

test("a step that fails to become a task keeps the ones made and says so once", async () => {
  const { resourceTestServer } = await import("$shared/testing/resources/server");
  const server = resourceTestServer("profile");
  let creates = 0;
  native.resource.mockImplementation(async (owner: string, call) => {
    if (call.kind === "mutate" && call.command.intent.kind === "create" && ++creates === 3)
      return { profile: owner, response: { kind: "error", error: "unavailable" } };
    return server.call(owner, call);
  });
  const { scene, objectives } = planScene();
  const run = objectives.get("objective")!.executions[0]!;
  const view = artifactView(run.artifacts[0]!, run);
  expect(scene.elements.map((element) => element.id)).toContain("plan-card");
  const plan = stepPlan("plan-card", "objective", view, resultPlan(view.content));
  const session = new TaskSession("profile");
  await session.start();
  const tasks = new WorkTasks("profile", session, () => new Map([["plan-card", plan]]), vi.fn());
  expect(await tasks.make("plan-card")).toBe(false);
  expect(server.records.size).toBe(2);
  expect(tasks.failed).toBe(true);
  expect(tasks.state("plan-card")).toBe("ready");
  // Asked again, only the missing steps are made.
  expect(await tasks.make("plan-card")).toBe(true);
  expect(server.records.size).toBe(4);
  expect(tasks.failed).toBe(false);
  expect(tasks.state("plan-card")).toBe("made");
  session.stop();
});
