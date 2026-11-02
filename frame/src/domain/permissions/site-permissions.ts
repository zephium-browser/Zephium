import type {
  SitePermissionDecisionView,
  SitePermissionKindView,
  SitePermissionsView,
} from "$shared/ipc/bindings";
import { commands } from "$shared/ipc/bindings";
import { operations, settle } from "$domain/operations";

/**
 * The focused profile's saved site decisions, of one origin when given; the
 * actor picks the profile. Null when they cannot be read right now.
 */
export async function list(origin?: string): Promise<SitePermissionsView | null> {
  try {
    const result = await commands.sitePermissions(origin ?? null);
    return result.status === "ok" ? result.data : null;
  } catch {
    return null;
  }
}

/** Changes one decision; null forgets it so the site asks again. */
export async function set(
  origin: string,
  kind: SitePermissionKindView,
  decision: SitePermissionDecisionView | null,
): Promise<boolean> {
  try {
    await operations.init();
  } catch {
    // The admission still settles; only its disposition may be missed.
  }
  const result = await settle(commands.sitePermissionSet(origin, kind, decision), 10_000);
  return result.outcome === "applied" || result.outcome === "no_op";
}
