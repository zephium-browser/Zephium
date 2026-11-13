import "$styles/global.css";
import { expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page, userEvent } from "vitest/browser";
import { WorkEnvironmentSession } from "$domain/work-environment";
import type {
  WorkCallV1,
  WorkEnvironmentSnapshot,
  WorkHumanPageV1,
  WorkHumanPhaseV1,
  WorkRuntimeProjection,
} from "$shared/ipc/bindings";
import WorkEnvironmentWorkspace from "../components/WorkEnvironmentWorkspace.svelte";
import { projection as base, snapshot as scene } from "./environment-fixtures";
const native = vi.hoisted(() => ({
  call: vi.fn(),
  activity: vi.fn(),
  humanPages: vi.fn(),
  present: vi.fn(),
  continue: vi.fn(),
  release: vi.fn(),
  paneHide: vi.fn(),
}));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockComposerBindings } = await import("./composer-fixtures");
  return mockComposerBindings({
    faviconProbe: async () => true,
    workCall: native.call,
    workActivity: native.activity,
    workHumanPages: native.humanPages,
    workHumanPresent: native.present,
    workHumanContinue: native.continue,
    workHumanRelease: native.release,
    workPaneHide: native.paneHide,
  });
});

const PROFILE = "profile";
const WORK = "objective";

function agentProjection(): WorkRuntimeProjection {
  const state = structuredClone(base);
  const run = state.executions[0]!;
  run.status = "running";
  run.spec.request = "Book the ferry";
  run.spec.nodes[0]!.capability = {
    kind: "agent",
    grant: {
      provider: "open_ai",
      model: "gpt-5.6-luna",
      max_turns: 10,
      max_steps: 32,
      browse_hops: 4,
    },
  };
  run.artifacts = [];
  run.user_artifacts = [];
  run.steps = [
    {
      id: "read",
      turn: 1,
      kind: { kind: "read", url: "https://ferry.example/book" },
      status: "running",
    },
  ];
  return state;
}

const heldPage = (phase: WorkHumanPhaseV1): WorkHumanPageV1 => ({
  id: { attempt: "attempt", step: "read", generation: 4 },
  phase,
  reason: "sign_in",
  remaining_millis: 150_000,
  document_revision: "9",
  can_continue: true,
  can_extend: false,
  work_memory: false,
});

test("a page waiting for a person is taken over in the pane, handed back, and released on Escape", async () => {
  const { emitNativeEvent } = await import("$shared/testing/native-events");
  await page.viewport(1200, 800);
  let phase: WorkHumanPhaseV1 = "waiting_for_human";
  const snapshot: WorkEnvironmentSnapshot = {
    ...structuredClone(scene),
    elements: [
      { id: "objective-card", area: null, reference: { kind: "objective", objective: WORK } },
    ],
  };
  native.call.mockImplementation(async (_profile: string, call: WorkCallV1) => {
    if (call.kind === "query" && call.request.query.kind === "projection")
      return {
        version: 1,
        profile: PROFILE,
        reply: { kind: "projection", projection: agentProjection() },
      };
    if (call.kind === "environment")
      return {
        version: 1,
        profile: PROFILE,
        reply: { kind: "environment", reply: { kind: "snapshot", snapshot } },
      };
    return { version: 1, profile: PROFILE, reply: { kind: "error", error: "not_found" } };
  });
  native.activity.mockResolvedValue({
    version: 1,
    profile: PROFILE,
    work: WORK,
    error: null,
    activity: [],
    pages: [
      {
        execution: "execution",
        attempt: "attempt",
        step: "read",
        url: "https://ferry.example/book",
        live: true,
        frame: null,
      },
    ],
  });
  const answer = () => ({
    version: 1,
    profile: PROFILE,
    work: WORK,
    accepted: true,
    pages: [heldPage(phase)],
    error: null,
  });
  native.humanPages.mockImplementation(async () => answer());
  native.present.mockImplementation(async () => answer());
  native.continue.mockImplementation(async () => answer());
  native.release.mockImplementation(async () => answer());
  const session = new WorkEnvironmentSession(PROFILE, scene.space);
  session.snapshot = snapshot;
  session.selected = scene.id;
  const screen = await render(WorkEnvironmentWorkspace, {
    session,
    tabs: [],
    spaceName: "Personal",
    aiEnabled: false,
    onopen: vi.fn(),
    onnewtab: vi.fn(),
  });
  const root = screen.container.querySelector(".environment") as HTMLElement;
  root.style.height = "720px";
  root.style.width = "1100px";

  await expect.element(screen.getByText("Sign in to ferry.example", { exact: true })).toBeVisible();
  // The card sits under the canvas chrome; the control is driven where it lives.
  const help = () =>
    screen.container.querySelector<HTMLElement>('.needs [data-variant="primary"]')!;
  help().click();
  await expect.poll(() => native.present.mock.calls.length).toBe(1);
  const region = native.present.mock.lastCall![3];
  expect(region.x).toBeGreaterThanOrEqual(0);
  expect(region.y).toBeGreaterThanOrEqual(0);
  expect(region.width).toBeGreaterThanOrEqual(64);
  expect(region.height).toBeGreaterThanOrEqual(64);
  expect(region.x + region.width).toBeLessThanOrEqual(8192);
  expect(region.y + region.height).toBeLessThanOrEqual(8192);
  expect(region.x + region.width).toBeLessThanOrEqual(window.innerWidth);
  expect(region.y + region.height).toBeLessThanOrEqual(window.innerHeight);
  expect(native.present.mock.lastCall![2]).toEqual({
    attempt: "attempt",
    step: "read",
    generation: 4,
  });
  const pane = screen.getByRole("region", { name: "Help the agent", exact: true });
  await expect.element(pane).toBeVisible();
  // The header says who, why in one sentence, and how long.
  await expect
    .element(pane.getByText("Sign in to ferry.example so the agent can continue", { exact: true }))
    .toBeVisible();
  await expect.element(pane.getByRole("button", { name: "Skip this page" })).toBeVisible();
  await expect.element(pane.getByText("The agent goes on without this page")).toBeVisible();

  // Escape gives the page straight back; nothing is left presented.
  await userEvent.keyboard("{Escape}");
  await expect.poll(() => native.release.mock.calls.length).toBeGreaterThanOrEqual(1);
  await expect.element(pane).not.toBeInTheDocument();

  help().click();
  await expect.poll(() => native.present.mock.calls.length).toBe(2);
  phase = "presented";
  emitNativeEvent("workHumanChanged", { profile: PROFILE, work: WORK });

  // The page is the person's own session: there is no account to choose.
  expect(pane.getByRole("radio").elements()).toHaveLength(0);
  await screen.getByRole("button", { name: "I’m done, continue", exact: true }).click();
  await expect.poll(() => native.continue.mock.calls.length).toBe(1);
  expect(native.continue.mock.lastCall![3]).toBe("anonymous");

  phase = "continuing";
  emitNativeEvent("workHumanChanged", { profile: PROFILE, work: WORK });
  await expect.element(screen.getByText("Handing back to the agent…")).toBeVisible();

  // The agent has it again: the pane closes itself and asks for no release.
  const released = native.release.mock.calls.length;
  phase = "reading";
  emitNativeEvent("workHumanChanged", { profile: PROFILE, work: WORK });
  await expect
    .element(screen.getByRole("region", { name: "Help the agent", exact: true }))
    .not.toBeInTheDocument();
  expect(native.release.mock.calls.length).toBe(released);
  await screen.unmount();
  session.dispose();
});

test("a page that needs no sign-in asks for no account and hands back anonymously", async () => {
  await page.viewport(1200, 800);
  const { default: TakeoverPane } = await import("../components/pane/TakeoverPane.svelte");
  const oncontinue = vi.fn();
  const onclose = vi.fn();
  const human = {
    attempt: "attempt",
    step: "read",
    generation: 4,
    phase: "presented" as const,
    reason: "challenge" as const,
    remaining: 30_000,
    canContinue: true,
    canExtend: false,
    workMemory: false,
  };
  const screen = await render(TakeoverPane, {
    host: "ferry.example",
    url: "https://ferry.example/book",
    bounds: new DOMRect(0, 0, 1200, 800),
    page: human,
    onregion: vi.fn(),
    oncontinue,
    onclose,
  });
  const header = screen.container.querySelector<HTMLElement>(".head")!;
  // The header: the host, why in one sentence, and the time left; nothing else.
  expect(header.querySelector(".who")?.textContent).toBe("ferry.example");
  expect(header.querySelector(".why")?.textContent).toBe(
    "ferry.example wants a person to prove they are not a robot",
  );
  expect(header.querySelector(".left")?.textContent).toBe("0:30 left");
  expect(header.querySelector("button")).toBeNull();
  expect(screen.container.querySelector('[role="radio"], input[type="radio"]')).toBeNull();
  expect(screen.container.textContent).not.toContain("Stay anonymous");
  await screen.getByRole("button", { name: "I’m done, continue" }).click();
  expect(oncontinue).toHaveBeenCalledExactlyOnceWith("anonymous");
  await expect.element(screen.getByText("The agent goes on without this page")).toBeVisible();
  await screen.getByRole("button", { name: "Skip this page" }).click();
  expect(onclose).toHaveBeenCalledOnce();
  await screen.unmount();
});
