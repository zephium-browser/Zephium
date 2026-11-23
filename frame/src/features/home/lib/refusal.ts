import * as m from "$shared/i18n/messages";
import type { Refusal } from "./model";

export const refusal = (reason: Refusal): string =>
  ({
    blocked: m.home_refused_blocked,
    challenge: m.home_refused_challenge,
    unreadable: m.home_refused_unreadable,
    unavailable: m.home_refused_unavailable,
  })[reason]();
