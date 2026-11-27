import * as m from "$shared/i18n/messages";
import type { WorkExecutionFact, WorkRuntimeProjection } from "$shared/ipc/bindings";
import { isLive } from "./agent-steps";
import type { DraftView } from "./board/types";

/**
 * Where a draft's Send stands, read from its durable grant or a known run: sending while the
 * run works or waits on Confirm, sent only when the page showed the send
 * happened, otherwise back to Send with what the run said.
 */
export function draftSend(
  projection: WorkRuntimeProjection,
  execution: WorkExecutionFact | string | undefined,
): Pick<DraftView, "send" | "note"> {
  if (typeof execution === "string") {
    const artifact = execution;
    execution = projection.executions.findLast((entry) =>
      entry.spec.nodes.some(
        (node) => node.capability.kind === "agent" && node.capability.grant.draft === artifact,
      ),
    );
  }
  if (!execution) return { send: "draft" };
  if (isLive(projection, execution)) return { send: "sending" };
  const steps = execution.steps ?? [];
  const sent = steps.findLast(
    (step) => step.kind.kind === "confirm" && step.kind.confirm.category === "communication",
  );
  if (sent?.status === "succeeded") return { send: "sent" };
  if (sent?.status === "outcome_unknown") return { send: "draft", note: m.work_draft_unverified() };
  if (sent?.status === "cancelled") return { send: "draft", note: m.work_draft_declined() };
  const said = sent?.note ?? steps.findLast((step) => step.kind.kind === "finish")?.note;
  return { send: "draft", note: said ?? m.work_draft_not_sent() };
}
