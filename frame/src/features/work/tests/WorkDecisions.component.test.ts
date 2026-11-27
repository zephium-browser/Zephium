import "$styles/global.css";
import { expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page } from "vitest/browser";
import { WorkEnvironmentSession } from "$domain/work-environment";
import type { WorkCallV1, WorkEnvironmentSnapshot } from "$shared/ipc/bindings";
import { tabFixture } from "$shared/testing/fixtures";
import WorkEnvironmentWorkspace from "../components/WorkEnvironmentWorkspace.svelte";
// Compile the lazy renderer before the interaction readiness deadline starts.
import "../components/WorkCanvas.svelte";
const native = vi.hoisted(() => ({ call: vi.fn(), preview: vi.fn() }));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockComposerBindings } = await import("./composer-fixtures");
  return mockComposerBindings({
    faviconProbe: async () => true,
    workCall: native.call,
    workContextPreview: native.preview,
  });
});

test("a decision is recorded on the element, shown on the card, and disclosed as context", async () => {
  await page.viewport(1200, 800);
  const profile = "00000000000000000000000001";
  const space = "00000000000000000000000002";
  let snapshot: WorkEnvironmentSnapshot = {
    version: 1,
    id: "00000000000000000000000003",
    profile,
    space,
    title: "Keyboards",
    lifecycle: "active",
    revision: "1",
    elements: [
      {
        id: "00000000000000000000000004",
        area: null,
        reference: { kind: "browser", tab: "tab-1" },
      },
    ],
    areas: [],
    view: {
      revision: "1",
      x: 0,
      y: 0,
      zoom_milli: 1000,
      placements: [
        { element: "00000000000000000000000004", x: 80, y: 80, width: 320, height: 200 },
      ],
    },
  };
  const intents: unknown[] = [];
  native.preview.mockResolvedValue({
    kind: "admitted",
    disclosure: {
      version: 1,
      environment: snapshot.id,
      environment_revision: "2",
      purpose: "planning",
      total_bytes: 40,
      items: [
        {
          element: "00000000000000000000000004",
          kind: "tab",
          title: "Keychron K2",
          revision: "https://example.com/",
          digest: "a".repeat(64),
          bytes: 30,
          truncated: false,
          visibility: "private",
        },
        {
          element: "00000000000000000000000004",
          kind: "decision",
          title: "Keychron K2",
          revision: "",
          digest: "b".repeat(64),
          bytes: 10,
          truncated: false,
          visibility: "private",
          implicit: true,
        },
      ],
    },
  });
  native.call.mockImplementation(async (_profile: string, call: WorkCallV1) => {
    if (call.kind !== "environment") throw new Error("unexpected call");
    const request = call.request;
    if (request.kind === "command" && request.intent.kind === "edit") {
      intents.push(request.intent.edit);
      const edit = request.intent.edit;
      if (edit.kind === "decide")
        snapshot = {
          ...snapshot,
          revision: "2",
          decisions: [{ element: edit.element, choice: edit.choice }],
        };
      if (edit.kind === "undecide") snapshot = { ...snapshot, revision: "3", decisions: [] };
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
    return {
      version: 1,
      profile,
      reply: { kind: "environment", reply: { kind: "snapshot", snapshot } },
    };
  });
  const session = new WorkEnvironmentSession(profile, space);
  session.snapshot = snapshot;
  session.selected = snapshot.id;
  const screen = await render(WorkEnvironmentWorkspace, {
    session,
    tabs: [tabFixture({ id: "tab-1", title: "Keychron K2" })],
    spaceName: "Personal",
    aiEnabled: true,
    onopen: vi.fn(),
    onnewtab: vi.fn(),
  });
  const root = screen.container.querySelector(".environment") as HTMLElement;
  root.style.height = "720px";
  root.style.width = "1100px";
  await expect
    .poll(() => screen.container.querySelectorAll(".work-drag-handle").length, { timeout: 8000 })
    .toBe(1);
  await screen.container.querySelector<HTMLElement>(".work-drag-handle")!.click();
  await screen.getByRole("button", { name: "Choose", exact: true }).click();
  await expect
    .poll(() => intents.at(-1))
    .toEqual({ kind: "decide", element: "00000000000000000000000004", choice: "Chosen" });
  await expect.poll(() => session.snapshot?.decisions?.length ?? 0).toBe(1);
  await expect.element(screen.getByText("Decided", { exact: true })).toBeVisible();
  // The selection rides with the ask only once the person asks: a chip in the field.
  expect(screen.container.querySelector(".context-manifest")).toBeNull();
  await screen.getByRole("button", { name: "Ask", exact: true }).click();
  await expect
    .element(screen.getByRole("textbox", { name: "What do you want to do?" }))
    .toHaveFocus();
  await expect
    .element(screen.getByRole("button", { name: /Using the selected object/ }))
    .toBeVisible();
  await screen.getByRole("button", { name: /Using the selected object/ }).click();
  await expect.element(screen.getByText("Decision", { exact: true }).first()).toBeVisible();
  await screen.container.querySelector<HTMLElement>(".work-drag-handle")!.click();
  await screen.getByRole("button", { name: "Unchoose", exact: true }).click();
  await expect
    .poll(() => intents.at(-1))
    .toEqual({
      kind: "undecide",
      element: "00000000000000000000000004",
    });
  await screen.unmount();
  session.dispose();
});
