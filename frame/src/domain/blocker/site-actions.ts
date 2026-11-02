import { commands } from "$shared/ipc/bindings";
import { boundedIpc, settleMutation, status } from "./blocker.svelte";
import type { BlockerMutationResult } from "./blocker.svelte";

export const changeSite = (
  context: import("$shared/ipc/bindings").BlockerSiteContext,
  action: import("$shared/ipc/bindings").BlockerSiteAction,
) => settleMutation(() => commands.blockerSiteChange(context, action));

const applied = (result: BlockerMutationResult) =>
  result.state === "processed" &&
  (result.disposition.outcome === "applied" || result.disposition.outcome === "no_op");

/**
 * Pauses or resumes blocking on the page in front. Requests the page already
 * made stay made, so it reloads for the switch to mean what it says.
 */
export async function setProtected(protect: boolean): Promise<boolean> {
  const context = status().site?.context;
  if (!context) return false;
  const done = applied(await changeSite(context, { kind: "pause", paused: !protect }));
  if (done) void commands.tabsReload(context.tab);
  return done;
}

export async function picker(
  context: import("$shared/ipc/bindings").BlockerSiteContext,
  action: import("$shared/ipc/bindings").BlockerPickerAction,
): Promise<import("$shared/ipc/bindings").BlockerPickerView | null> {
  try {
    const result = await boundedIpc(commands.blockerPicker(context, action), 4_000);
    return result.status === "ok" ? result.data : null;
  } catch {
    return null;
  }
}
