import { commands } from "$shared/ipc/bindings";
import type {
  BrowsingDataCall,
  BrowsingDataResponse,
  BrowsingDataRange,
  SiteDataView,
} from "$shared/ipc/bindings";

/** What a clear removes. `cookies` is cookies and site data; `downloads` is
 *  the download list, never the files. */
export type ClearKinds = {
  history: boolean;
  cookies: boolean;
  cache: boolean;
  downloads: boolean;
};

/** `private` means the window keeps nothing to clear. */
export type ClearOutcome = "cleared" | "private" | "failed";

export type SiteList = {
  sites: SiteDataView[];
  /** Only some of what the engine holds can be listed by site. */
  partial: boolean;
};

async function call(profile: string, request: BrowsingDataCall): Promise<BrowsingDataResponse> {
  try {
    return await commands.browsingDataCall(profile, request);
  } catch {
    return { kind: "error", error: "unavailable" };
  }
}

function outcome(response: BrowsingDataResponse): ClearOutcome {
  if (response.kind === "cleared") return "cleared";
  return response.kind === "error" && response.error === "private" ? "private" : "failed";
}

export async function clearBrowsingData(
  profile: string,
  range: BrowsingDataRange,
  kinds: ClearKinds,
): Promise<ClearOutcome> {
  return outcome(await call(profile, { kind: "clear", range, ...kinds }));
}

/** The sites the profile holds cookies and site data for, or null when they
 *  could not be listed. */
export async function listSites(profile: string): Promise<SiteList | null> {
  const response = await call(profile, { kind: "sites" });
  return response.kind === "sites" ? { sites: response.sites, partial: response.partial } : null;
}

/** Removes everything stored for one listed site. */
export async function forgetSite(profile: string, site: string): Promise<ClearOutcome> {
  return outcome(await call(profile, { kind: "forget_site", site }));
}

/** Removes the cookies and site data of every site, whenever stored. */
export function forgetAllSites(profile: string): Promise<ClearOutcome> {
  return clearBrowsingData(profile, "everything", {
    history: false,
    cookies: true,
    cache: false,
    downloads: false,
  });
}

/** The origin and host of a web page, or null for an address with no site
 *  data to clear. */
export function pageSite(url: string | null | undefined): { origin: string; host: string } | null {
  if (!url) return null;
  try {
    const parsed = new URL(url);
    if (parsed.protocol !== "https:" && parsed.protocol !== "http:") return null;
    return { origin: parsed.origin, host: parsed.hostname };
  } catch {
    return null;
  }
}

/** Removes everything the profile stored for the site of a page: its cookies
 *  and the storage of its origins. Open tabs keep running. A site-info
 *  popover calls this once the person has confirmed. */
export async function clearDataForPage(profile: string, url: string): Promise<ClearOutcome> {
  const site = pageSite(url);
  return site === null
    ? "failed"
    : outcome(await call(profile, { kind: "forget_page", origin: site.origin }));
}
