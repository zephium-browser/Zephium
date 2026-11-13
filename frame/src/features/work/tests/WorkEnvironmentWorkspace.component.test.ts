import "$styles/global.css";
import { expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page, userEvent } from "vitest/browser";
import { WorkEnvironmentSession } from "$domain/work-environment";
import { AGENT_GRANT, AGENT_LIMITS } from "$domain/work";
import type { WorkCallV1, WorkEnvironmentSnapshot } from "$shared/ipc/bindings";
import { tabFixture } from "$shared/testing/fixtures";
import WorkEnvironmentWorkspace from "../components/WorkEnvironmentWorkspace.svelte";
const native = vi.hoisted(() => ({
  call: vi.fn(),
  paneShow: vi.fn(),
  paneHide: vi.fn(),
  paneRect: vi.fn(),
}));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    faviconProbe: async () => true,
    workCall: native.call,
    workPaneShow: native.paneShow,
    workPaneHide: native.paneHide,
    workPaneSetRect: native.paneRect,
  });
});

test("the production manual environment attaches a real tab, opens it in the pane, and hands off to Browse only explicitly", async () => {
  const { emitNativeEvent } = await import("$shared/testing/native-events");
  await page.viewport(1200, 800);
  native.paneShow.mockResolvedValue({ accepted: true, operation_id: null });
  const profile = "00000000000000000000000001";
  const space = "00000000000000000000000002";
  const id = "00000000000000000000000003";
  let snapshot: WorkEnvironmentSnapshot = {
    version: 1,
    id,
    profile,
    space,
    title: "Manual research",
    lifecycle: "active",
    revision: "1",
    elements: [],
    areas: [],
    view: { revision: "1", x: 0, y: 0, zoom_milli: 1000, placements: [] },
  };
  native.call.mockImplementation(async (_profile: string, call: WorkCallV1) => {
    if (call.kind !== "environment")
      throw new Error("Manual Work must not initialize objective execution");
    const request = call.request;
    if (request.kind === "checkpoint") {
      snapshot = {
        ...snapshot,
        view: { ...request.view, revision: String(BigInt(request.expected) + 1n) },
      };
      return {
        version: 1,
        profile,
        reply: {
          kind: "environment",
          reply: {
            kind: "checkpointed",
            expected: request.expected,
            applied_view_revision: snapshot.view.revision,
            replayed: false,
            snapshot,
          },
        },
      };
    }
    if (request.kind === "command") {
      if (request.intent.kind === "edit" && request.intent.edit.kind === "add") {
        snapshot = {
          ...snapshot,
          revision: "2",
          elements: [
            {
              id: "00000000000000000000000004",
              area: null,
              reference: request.intent.edit.reference,
            },
          ],
        };
      }
      return {
        version: 1,
        profile,
        reply: {
          kind: "environment",
          reply: {
            kind: "applied",
            command: request.command,
            applied_revision: snapshot.revision,
            applied_view_revision: snapshot.view.revision,
            replayed: false,
            snapshot,
          },
        },
      };
    }
    return {
      version: 1,
      profile,
      reply: { kind: "environment", reply: { kind: "snapshot", snapshot } },
    };
  });
  const session = new WorkEnvironmentSession(profile, space);
  session.snapshot = snapshot;
  session.selected = id;
  const onopen = vi.fn();
  const screen = await render(WorkEnvironmentWorkspace, {
    session,
    tabs: [tabFixture({ id: "retained-tab", title: "Research tab" })],
    spaceName: "Personal",
    aiEnabled: false,
    onopen,
    onnewtab: vi.fn(),
  });
  const root = screen.container.querySelector(".environment") as HTMLElement;
  root.style.height = "720px";
  root.style.width = "1100px";
  await screen.getByRole("button", { name: "Add to canvas", exact: true }).click();
  await screen.getByRole("checkbox", { name: /Research tab/ }).click();
  await screen.getByRole("button", { name: "Add to Work (1)" }).click();
  await expect
    .poll(() => session.snapshot?.elements[0]?.reference)
    .toEqual({ kind: "browser", tab: "retained-tab" });
  expect(onopen).not.toHaveBeenCalled();
  await screen.getByRole("button", { name: "Add to canvas", exact: true }).click();
  await expect.poll(() => screen.container.querySelectorAll(".work-drag-handle").length).toBe(1);
  await screen.container.querySelector<HTMLElement>(".work-drag-handle")!.click();
  await screen.getByRole("button", { name: "Open", exact: true }).click();
  await expect.poll(() => native.paneShow.mock.calls.length).toBe(1);
  expect(native.paneShow.mock.lastCall?.[0]).toEqual({ kind: "tab", id: "retained-tab" });
  const hole = native.paneShow.mock.lastCall?.[1];
  expect(hole.width).toBeGreaterThanOrEqual(480);
  expect(onopen).not.toHaveBeenCalled();
  const row = screen.container.querySelector<HTMLElement>("[data-zephium-tab-id]")!;
  expect(row.dataset.zephiumTabId).toBe("retained-tab");
  emitNativeEvent("layoutChanged", {
    dividers: [],
    work_pane: { tab: "retained-tab", ...hole, presented: true, generation: 1 },
    glance: null,
  });
  await expect.element(screen.getByRole("toolbar", { name: "Move pane" })).toBeVisible();
  const handoff = screen
    .getByRole("region", { name: "Browser pane", exact: true })
    .getByRole("button", { name: "Open in Browse", exact: true });
  await expect.element(handoff).toBeEnabled();
  await handoff.click();
  await expect.poll(() => onopen.mock.calls).toEqual([["retained-tab"]]);
  emitNativeEvent("layoutChanged", { dividers: [], work_pane: null, glance: null });
  await expect
    .element(screen.getByRole("region", { name: "Browser pane", exact: true }))
    .not.toBeInTheDocument();
  await expect
    .element(screen.getByRole("textbox", { name: "What do you want to do?" }))
    .not.toBeInTheDocument();
  await screen.unmount();
  session.dispose();
});

test("opening an objective lifts it over the canvas and returns on close", async () => {
  const { workSession } = await import("$domain/work");
  const { projection, snapshot } = await import("./environment-fixtures");
  const environment = new WorkEnvironmentSession(snapshot.profile, snapshot.space);
  environment.snapshot = structuredClone(snapshot);
  environment.selected = snapshot.id;
  const objective = workSession(snapshot.profile)!;
  vi.spyOn(objective, "start").mockResolvedValue();
  vi.spyOn(objective, "open").mockImplementation(async () => {
    objective.projection = structuredClone(projection);
    return true;
  });
  native.call.mockResolvedValue({
    version: 1,
    profile: snapshot.profile,
    reply: { kind: "error", error: "not_found" },
  });
  const screen = await render(WorkEnvironmentWorkspace, {
    session: environment,
    tabs: [],
    spaceName: "Personal",
    aiEnabled: false,
    onopen: vi.fn(),
    onnewtab: vi.fn(),
  });
  const root = screen.container.querySelector(".environment") as HTMLElement;
  root.style.height = "720px";
  root.style.width = "1100px";
  await expect.poll(() => screen.container.querySelectorAll(".work-drag-handle").length).toBe(1);
  await screen.container.querySelector<HTMLElement>(".work-drag-handle")!.click();
  await screen.getByRole("button", { name: "Open", exact: true }).click();
  const panel = screen.getByRole("dialog", { name: "Request", exact: true });
  await expect.element(panel).toBeVisible();
  await expect.element(panel).toHaveFocus();
  expect(screen.container.querySelector(".inspector")).toBeNull();
  await userEvent.keyboard("{Escape}");
  await expect.element(panel).not.toBeInTheDocument();
  expect(screen.container.querySelector(".inspector")).toBeNull();
  await screen.unmount();
  environment.dispose();
  objective.dispose();
});

test("prompt submission keeps work on canvas and clarification choices above the available composer", async () => {
  await page.viewport(1100, 750);
  const { workSession } = await import("$domain/work");
  const { projection, snapshot } = await import("./environment-fixtures");
  const environment = new WorkEnvironmentSession("interaction-profile", "space");
  environment.snapshot = { ...snapshot, profile: "interaction-profile", elements: [] };
  const objective = workSession("interaction-profile")!;
  vi.spyOn(objective, "start").mockResolvedValue();
  vi.spyOn(objective, "open").mockResolvedValue(true);
  const create = vi.spyOn(objective, "create").mockImplementation(async (text) => {
    objective.selected = "objective";
    objective.projection = {
      ...projection,
      executions: [],
      work: {
        ...projection.work,
        profile: "interaction-profile",
        status: "draft",
        objective: text,
      },
    };
    return true;
  });
  const planning = vi.spyOn(objective.operations, "begin").mockImplementation(async (input) => ({
    id: "operation",
    input,
    state: { kind: "pending", work: "objective" },
  }));
  vi.spyOn(environment, "edit").mockImplementation(async (edit) => {
    expect(edit).toMatchObject({
      kind: "add",
      reference: { kind: "objective", objective: "objective" },
    });
    environment.snapshot = { ...environment.snapshot!, elements: [snapshot.elements[0]!] };
    return true;
  });
  native.call.mockResolvedValue({
    version: 1,
    profile: "interaction-profile",
    reply: { kind: "error", error: "not_found" },
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
  root.style.height = "600px";
  root.style.width = "1100px";
  const composer = screen.getByRole("textbox", {
    name: "What do you want to do?",
    exact: true,
  });
  await expect.element(screen.getByRole("button", { name: "Fit view", exact: true })).toBeVisible();
  const longObjective =
    "Find a keyboard with quiet switches and compare available options, delivery dates, prices, and compatibility for my workspace.";
  await composer.fill(longObjective);
  await screen.getByRole("button", { name: "Send", exact: true }).click();
  await expect.poll(() => create.mock.calls).toEqual([[longObjective, expect.any(String)]]);
  await expect
    .poll(() => planning.mock.calls)
    .toEqual([
      [
        {
          kind: "run",
          command: {
            version: 1,
            work: "objective",
            expected_revision: "4",
            command: expect.any(String),
            intent: { kind: "begin_agent", grant: AGENT_GRANT, limits: AGENT_LIMITS },
          },
        },
      ],
    ]);
  expect(screen.container.querySelector(".detail")).toBeNull();
  expect(screen.container.querySelector(".approval")).toBeNull();
  const asking = structuredClone(projection.executions[0]!);
  asking.status = "running";
  asking.authorization = "user_directed_agent";
  asking.attempts = [{ id: "attempt", node: "node", status: "running", usage: null }];
  asking.spec.nodes[0]!.capability = {
    kind: "agent",
    grant: {
      provider: "open_ai",
      model: "gpt-5.6-luna",
      max_turns: 10,
      max_steps: 32,
      browse_hops: 4,
    },
  };
  asking.steps = [
    {
      id: "ask-1",
      turn: 1,
      kind: { kind: "ask", prompt: "Choose a budget", options: ["Under 150", "Under 300"] },
      status: "running",
    },
  ];
  objective.projection = { ...objective.projection!, executions: [asking] };
  // The question stands in the island as its card, above the bar.
  await expect
    .element(screen.getByRole("button", { name: "Under 150", exact: true }))
    .toBeVisible();
  // One field, two meanings: while the run asks, the same field adds to it.
  await expect
    .element(screen.getByRole("textbox", { name: "Add to this…", exact: true }))
    .toBeVisible();
  const question = screen.container.querySelector(".agent-line .ask input")!;
  const prompt = screen.container.querySelector(".work-bar textarea")!;
  expect(question.compareDocumentPosition(prompt) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  expect(question.getBoundingClientRect().bottom).toBeLessThan(prompt.getBoundingClientRect().top);
  const answerStep = vi.spyOn(objective, "answerStep").mockResolvedValue(true);
  await screen.getByRole("button", { name: "Under 150", exact: true }).click();
  await expect.poll(() => answerStep.mock.calls.length).toBe(1);
  expect(answerStep).toHaveBeenCalledExactlyOnceWith("execution", "ask-1", "Under 150");
  expect(planning).toHaveBeenCalledTimes(1);
  const link = { extraction_id: "provider-record", source_id: 1 };
  const completed = structuredClone(projection.executions[0]!);
  completed.user_artifacts![0]!.evidence = [link];
  completed.spec.nodes.push({ ...completed.spec.nodes[0]!, node: "child-node", parent: "node" });
  completed.artifacts.push({
    ...completed.artifacts[0]!,
    id: "supporting-result",
    node: "child-node",
    title: "Supporting research",
  });
  objective.projection = {
    ...objective.projection!,
    work: { ...objective.projection!.work, questions: [], status: "plan_ready" },
    executions: [completed],
  };
  root.style.height = "750px";
  const readEvidence = vi.spyOn(objective, "evidence").mockResolvedValue({
    version: 1,
    link,
    origin: "example.com",
    role: "citation",
    text: "Retained source",
    truncated: false,
    source_bytes: "15",
    source: {
      kind: "provider_search",
      provider: "open_ai",
      model: "search-model",
      url: "https://example.com/source",
      title: "Exact cited source",
      response_id: "response",
      search_call_id: "search-call",
    },
  });
  // The zoom reads at the canvas's bottom edge; the canvas draws no top toolbar.
  expect(screen.container.querySelector("[data-work-zoom-slot] [role='group']")).not.toBeNull();
  expect(screen.container.querySelector("[data-zephium-work-chrome]")).toBeNull();
  await screen.getByRole("button", { name: "Fit view", exact: true }).click();
  await expect.element(screen.getByText("Reviewed findings", { exact: true })).toBeVisible();
  expect(screen.container.querySelector(".detail")).toBeNull();
  // The card carries the answer; its citations live in the lift, named by kind, never numbered.
  expect(screen.container.querySelector(".work-canvas .chip")).toBeNull();
  // A reviewed plan's result opens in the lift, where it can be reviewed.
  await screen.getByText("Reviewed findings", { exact: true }).dblClick();
  // The lift names its sources in one collapsed rail, not chips under the text.
  await screen.getByRole("button", { name: "Based on 1 source" }).click();
  await screen.getByRole("button", { name: "Page", exact: true }).first().click();
  await expect.element(screen.getByText("Exact cited source", { exact: true })).toBeVisible();
  expect(readEvidence).toHaveBeenCalledExactlyOnceWith(link);
  // A result reads as finished: no review strip beside it, no second panel.
  await expect
    .element(screen.getByRole("button", { name: "Edit result", exact: true }))
    .not.toBeInTheDocument();
  expect(screen.container.querySelector(".detail")).toBeNull();
  await screen.getByRole("button", { name: "Back to canvas", exact: true }).click();
  await expect
    .poll(() => screen.container.querySelector(".agent-line.settled") !== null)
    .toBe(true);
  // The settled line is the island at the canvas's top, one line tall.
  await expect
    .poll(() => screen.container.querySelector(".island")!.getBoundingClientRect().height)
    .toBeLessThan(60);
  await screen.unmount();
  environment.dispose();
  objective.dispose();
});
