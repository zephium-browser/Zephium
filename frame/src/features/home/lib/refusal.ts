import * as m from "$shared/i18n/messages";
import type { Refusal } from "$domain/home";

/** Why a page or place could not be read, in the person's words. */
export function refusal(reason: Refusal): string {
  switch (reason) {
    case "blocked":
      return m.home_refused_blocked();
    case "challenge":
    case "consent":
      return m.home_refused_challenge();
    case "unreadable":
    case "no_results":
    case "missing":
      return m.home_refused_unreadable();
    case "gone":
      return m.home_refused_gone();
    case "busy":
      return m.home_refused_busy();
    default:
      return m.home_refused_unavailable();
  }
}
