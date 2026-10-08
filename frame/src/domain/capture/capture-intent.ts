import { commands } from "$shared/ipc/bindings";
import { settle } from "$domain/operations";

async function stopCapture(item: string, navigation: string): Promise<boolean> {
  const result = await settle(commands.captureStop(item, navigation));
  return result.outcome !== "failed" && result.outcome !== "rejected";
}

/** Echo the immutable owner shown by this control, even if its tab changes. */
export function stopCaptureFor(item: string, navigation: string): () => Promise<boolean> {
  return () => stopCapture(item, navigation);
}
