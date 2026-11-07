import { pageSite } from "./site-data";

/** The page whose data a person was asked to clear. A tab menu command, and
 *  later a site-info popover, opens the confirmation by asking here. */
type ClearRequest = { url: string; host: string };

let request = $state.raw<ClearRequest | null>(null);

export const pending = () => request;

/** Asks to clear the data of the site a page is on; false when the address
 *  has none. */
export function ask(url: string | null | undefined): boolean {
  const site = pageSite(url);
  if (!url || !site) return false;
  request = { url, host: site.host };
  return true;
}

export function dismiss() {
  request = null;
}
