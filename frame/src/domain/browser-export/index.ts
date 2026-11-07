import { commands } from "$shared/ipc/bindings";
import type { ExportKindView, ExportOutcomeView } from "$shared/ipc/bindings";

export type ExportKind = ExportKindView;
/** `cancelled` is the person closing the save dialog; `private` means the
 *  window keeps nothing to export. */
export type ExportOutcome = ExportOutcomeView;

/** Native reads the focused profile, asks where to save, and writes the file. */
export async function exportBrowserData(kind: ExportKind): Promise<ExportOutcome> {
  try {
    return await commands.exportBrowserData(kind);
  } catch {
    return "failed";
  }
}
