import type { SitePermissionDecisionView, SitePermissionKindView } from "$shared/ipc/bindings";

/** What a row's control shows. Ask is no saved decision. */
export type PermissionChoice = "allow" | "block" | "ask";

type Capture = { camera: string; microphone: string };

/** Pop-ups are blocked unless a site is allowed, so with no decision they read
 *  as Block; every other kind with none asks. */
export function choiceOf(
  kind: SitePermissionKindView,
  decision: SitePermissionDecisionView | undefined,
): PermissionChoice {
  return decision ?? (kind === "popups" ? "block" : "ask");
}

/** The decision to save for a choice; null forgets it so the site asks, or for
 *  pop-ups stays blocked. */
export function decisionOf(
  kind: SitePermissionKindView,
  choice: PermissionChoice,
): SitePermissionDecisionView | null {
  if (choice === "ask" || (kind === "popups" && choice === "block")) return null;
  return choice;
}

/** The kinds a page in front is using right now. */
export function kindsInUse(capture: Capture | null | undefined): Set<SitePermissionKindView> {
  const kinds = new Set<SitePermissionKindView>();
  if (capture && capture.camera !== "none") kinds.add("camera");
  if (capture && capture.microphone !== "none") kinds.add("microphone");
  return kinds;
}

/**
 * The kinds worth a row: ones the site has a decision for, ones it is using,
 * and ones just changed, so a row does not vanish under the pointer when it
 * is set back to Ask. Platform order.
 */
export function shownKinds(
  kinds: readonly SitePermissionKindView[],
  ...groups: ReadonlyArray<ReadonlySet<SitePermissionKindView>>
): SitePermissionKindView[] {
  return kinds.filter((kind) => groups.some((group) => group.has(kind)));
}
