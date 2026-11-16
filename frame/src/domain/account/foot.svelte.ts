import * as accountSession from "./account.svelte";
import { avatarUrl, initials, planName, usageTone } from "./account-model";
import { preferences } from "$domain/preferences";
import { drawn, profiles } from "$domain/profiles";

/**
 * What the account surfaces show: the profile's picture for a foot's trigger,
 * and the account (the same in every profile) wherever it is summed up. One
 * reading of both, so the places that show it cannot drift. Call it during
 * component initialisation.
 */
export function accountFoot(options: { private?: () => boolean } = {}) {
  const account = $derived(accountSession.account());
  const signedIn = $derived(account?.signed_in === true);
  const used = $derived(signedIn ? (account?.usage?.used_percent ?? null) : null);
  /** Only the picture the profile chose; the account never stands in for it. */
  const face = $derived.by((): ReturnType<typeof drawn> | null => {
    if (options.private?.()) return null;
    const current = profiles.current();
    return current && current.avatar.revision > 0 ? drawn(current) : null;
  });
  // The same test Settings applies before it offers credits for sale.
  const canTopUp = $derived(
    signedIn &&
      account?.billing === true &&
      !account.plan_change &&
      !account.billing_review &&
      preferences.aiEnabled() &&
      account.offers.some((offer) => offer.kind === "top_up"),
  );

  return {
    /** The account service answers here, and this is not a private window. */
    get available() {
      return account?.available === true && !options.private?.();
    },
    get signedIn() {
      return signedIn;
    },
    get email() {
      return account?.email ?? null;
    },
    /** The first name the account itself carries, never one guessed from the address. */
    get name() {
      const first = signedIn ? (account?.name ?? "").trim().split(/\s+/u)[0] : "";
      return first || null;
    },
    get initials() {
      return signedIn ? initials(account?.email, account?.name) : "";
    },
    /** The account's own picture, shown where its initials would be. */
    get avatar() {
      return signedIn ? avatarUrl(account?.avatar) : null;
    },
    get plan() {
      return signedIn ? planName(account?.plan) : null;
    },
    get used() {
      return used;
    },
    /** Whether usage has been read at all, as opposed to read and not included. */
    get metered() {
      return signedIn && account?.usage != null;
    },
    get tone() {
      return usageTone(used);
    },
    get canTopUp() {
      return canTopUp;
    },
    get face() {
      return face;
    },
  };
}
