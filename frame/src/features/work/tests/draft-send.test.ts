import { describe, expect, test } from "vitest";
import type {
  WorkExecutionFact,
  WorkRuntimeProjection,
  WorkStepStatus,
} from "$shared/ipc/bindings";
import { draftSend } from "../lib/draft-send";
import { slackSend, runWith } from "./ask-fixtures";
import { projection } from "./environment-fixtures";

const state = (execution: WorkExecutionFact): WorkRuntimeProjection => ({
  ...projection,
  executions: [execution],
});
const sentAs = (status: WorkStepStatus, note?: string) =>
  runWith([{ ...slackSend, status, ...(note ? { note } : {}) }]);
const finished = (execution: WorkExecutionFact, status: WorkExecutionFact["status"]) => ({
  ...execution,
  status,
});

describe("a draft's send follows its run", () => {
  test("it is Sending while the run works or waits on the confirmation", () => {
    const waiting = sentAs("running");
    expect(draftSend(state(waiting), waiting).send).toBe("sending");
  });

  test("it is Sent only when the send step committed", () => {
    const done = finished(sentAs("succeeded"), "completed");
    expect(draftSend(state(done), done)).toEqual({ send: "sent" });
  });

  test("a send that ran unverified, was declined or failed goes back to Send with a note", () => {
    const unverified = finished(sentAs("outcome_unknown"), "completed");
    expect(draftSend(state(unverified), unverified)).toMatchObject({ send: "draft" });
    expect(draftSend(state(unverified), unverified).note).toBeTruthy();
    const declined = finished(sentAs("cancelled"), "completed");
    expect(draftSend(state(declined), declined)).toMatchObject({ send: "draft" });
    const failed = finished(
      sentAs("failed", "The page changed before it ran; nothing was sent"),
      "completed",
    );
    expect(draftSend(state(failed), failed)).toEqual({
      send: "draft",
      note: "The page changed before it ran; nothing was sent",
    });
  });

  test("a run that stopped before any send is not a send", () => {
    const stopped = finished(runWith([]), "cancelled");
    expect(draftSend(state(stopped), stopped).send).toBe("draft");
    expect(draftSend(state(stopped), stopped).note).toBeTruthy();
  });

  test("a draft without a run stays a draft", () => {
    expect(draftSend(projection, undefined)).toEqual({ send: "draft" });
  });

  test.each([
    ["running", "running", "sending"],
    ["completed", "succeeded", "sent"],
    ["completed", "outcome_unknown", "draft"],
    ["completed", "cancelled", "draft"],
  ] as const)("a restored draft finds its durable grant (%s, %s)", (status, step, expected) => {
    const run = finished(sentAs(step), status);
    const capability = run.spec.nodes[0]!.capability;
    if (capability.kind === "agent") capability.grant.draft = "draft-artifact";
    const unrelated = finished(sentAs("succeeded"), "completed");
    unrelated.id = "unrelated-run";
    const restored = structuredClone({ ...projection, executions: [run, unrelated] });
    expect(draftSend(restored, "draft-artifact").send).toBe(expected);
    expect(draftSend(restored, "another-draft")).toEqual({ send: "draft" });
  });
});
