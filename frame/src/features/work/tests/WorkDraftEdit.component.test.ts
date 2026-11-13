import "$styles/global.css";
import { expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page, userEvent } from "vitest/browser";
import ObjectsSheet from "./ObjectsSheet.svelte";
// Compile the lazy renderer before the interaction readiness deadline starts.
import "../components/objects/Draft.svelte";
import type { DraftView } from "../lib/board/types";
import type {
  WorkCallV1,
  WorkRuntimeProjection_Deserialize as WorkRuntimeProjection,
} from "$shared/ipc/bindings";
import { WorkEnvironmentSession } from "$domain/work-environment";
import { workSession } from "$domain/work";
import WorkEnvironmentWorkspace from "../components/WorkEnvironmentWorkspace.svelte";
import { projection, snapshot } from "./environment-fixtures";
import { runWith, slackSend } from "./ask-fixtures";

const native = vi.hoisted(() => ({ work: vi.fn() }));

vi.mock("$shared/ipc/bindings", async () => {
  const { mockComposerBindings } = await import("./composer-fixtures");
  return mockComposerBindings({ faviconProbe: async () => true, workCall: native.work });
});

const draft: DraftView = {
  kind: "draft",
  id: "review",
  destination: "github",
  to: "crynta/terax#1337",
  body: "Request changes: the copy path skips the deny-list.",
  send: "draft",
};

test("a draft is edited where it stands, takes its review and sends through its action", async () => {
  await page.viewport(1200, 800);
  const write = vi.fn();
  const review = vi.fn();
  const send = vi.fn();
  const screen = await render(ObjectsSheet, {
    rows: [{ object: draft, width: 520 }],
    actions: { write, review, send },
  });
  await screen.getByRole("button", { name: "Edit the draft" }).first().click();
  const field = screen.getByRole("textbox").first();
  await expect.element(field).toHaveFocus();
  await userEvent.keyboard(" Please fix.");
  await userEvent.keyboard("{Escape}");
  expect(write).toHaveBeenCalledWith("review", `${draft.body} Please fix.`);
  await screen.getByRole("radio", { name: "Request changes" }).first().click();
  expect(review).toHaveBeenCalledWith("review", "request_changes");
  await screen.getByRole("button", { name: "Comment", exact: true }).first().click();
  expect(send).toHaveBeenCalledWith("review");
});

test("a draft's send reads from its run: sending holds the button, a failure returns it with the note", async () => {
  await page.viewport(1200, 800);
  const send = vi.fn();
  const sending = await render(ObjectsSheet, {
    rows: [{ object: { ...draft, send: "sending" }, width: 520 }],
    actions: { send },
  });
  await expect.element(sending.getByRole("button", { name: "Sending…" }).first()).toBeDisabled();
  await sending.unmount();
  const failed = await render(ObjectsSheet, {
    rows: [{ object: { ...draft, send: "draft", note: "Not sent: you declined it." }, width: 520 }],
    actions: { send },
  });
  await expect.element(failed.getByText("Not sent: you declined it.").first()).toBeVisible();
  await failed.getByRole("button", { name: "Comment", exact: true }).first().click();
  expect(send).toHaveBeenCalledWith("review");
});

test("remounting the workspace keeps typed draft words and rebuilds Sent from the durable grant", async () => {
  await page.viewport(1200, 800);
  const state: WorkRuntimeProjection = structuredClone(projection);
  const producer = state.executions[0]!;
  producer.spec = runWith([]).spec;
  producer.user_artifacts = [];
  producer.artifacts[0]!.data = { kind: "draft", destination: "slack", body: "Original words" };
  const environment = new WorkEnvironmentSession(snapshot.profile, snapshot.space);
  environment.snapshot = structuredClone(snapshot);
  environment.selected = snapshot.id;
  const objective = workSession(snapshot.profile);
  vi.spyOn(objective, "start").mockResolvedValue();
  vi.spyOn(objective, "open").mockImplementation(async (id: string) => {
    objective.selected = id;
    objective.projection = structuredClone(state);
    return true;
  });
  native.work.mockImplementation(async (_profile: string, call: WorkCallV1) => {
    if (call.kind === "query" && call.request.query.kind === "projection")
      return {
        version: 1,
        profile: snapshot.profile,
        reply: { kind: "projection", projection: structuredClone(state) },
      };
    if (call.kind === "execute" && call.command.intent.kind === "edit_artifact") {
      expect(call.command.expected_revision).toBe(state.work.revision);
      state.work.revision = "5";
      producer.user_artifacts = [
        {
          artifact: call.command.intent.artifact,
          revision: "5",
          decision: "accepted",
          edited_data: call.command.intent.data,
          evidence: call.command.intent.evidence,
        },
      ];
      return {
        version: 1,
        profile: snapshot.profile,
        reply: {
          kind: "execution_applied",
          projection: structuredClone(state),
          receipt: { command: call.command.command, execution: producer.id, applied_revision: "5" },
        },
      };
    }
    return { version: 1, profile: snapshot.profile, reply: { kind: "error", error: "not_found" } };
  });
  const words = "Edited words kept even without blurring";
  const send = vi
    .spyOn(objective, "continueWith")
    .mockImplementation(async (_text, _context, options) => {
      expect(options?.draft).toBe("artifact");
      expect(producer.user_artifacts?.[0]?.edited_data).toMatchObject({ body: words });
      const run = runWith([{ ...slackSend, status: "succeeded" }]);
      run.id = "send-run";
      run.status = "completed";
      run.attempts = [];
      run.artifacts = [];
      run.user_artifacts = [];
      const capability = run.spec.nodes[0]!.capability;
      if (capability.kind === "agent") capability.grant.draft = options!.draft;
      state.executions.push(run);
      state.work.revision = "6";
      objective.projection = structuredClone(state);
      return true;
    });
  const mount = async () => {
    const screen = await render(WorkEnvironmentWorkspace, {
      session: environment,
      tabs: [],
      spaceName: "Personal",
      aiEnabled: true,
      onopen: vi.fn(),
      onnewtab: vi.fn(),
    });
    const root = screen.container.querySelector<HTMLElement>(".environment")!;
    root.style.height = "720px";
    root.style.width = "1100px";
    const cover = () => screen.container.querySelector<HTMLElement>('[data-card-id="result-card"]');
    await expect.poll(cover, { timeout: 5000 }).not.toBeNull();
    cover()!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    cover()!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await expect.element(screen.getByRole("dialog", { name: "Dependency findings" })).toBeVisible();
    return screen;
  };
  const first = await mount();
  await first.getByRole("textbox", { name: "Slack", exact: true }).fill(words);
  await first.unmount();
  const second = await mount();
  await expect
    .element(second.getByRole("textbox", { name: "Slack", exact: true }))
    .toHaveValue(words);
  await second
    .getByRole("dialog", { name: "Dependency findings" })
    .getByRole("button", { name: "Send", exact: true })
    .click();
  await expect.poll(() => send.mock.calls.length).toBe(1);
  await expect
    .element(
      second
        .getByRole("dialog", { name: "Dependency findings" })
        .getByRole("button", { name: "Sent", exact: true }),
    )
    .toBeDisabled();
  await second.unmount();
  const third = await mount();
  // The lift hides the canvas copy; check the sent words in the opened draft.
  const restored = third.getByRole("dialog", { name: "Dependency findings" });
  await expect.element(restored.getByText(words, { exact: true })).toBeVisible();
  await expect.element(restored.getByRole("button", { name: "Sent", exact: true })).toBeDisabled();
  expect(send).toHaveBeenCalledOnce();
  await third.unmount();
  environment.dispose();
  objective.dispose();
});
