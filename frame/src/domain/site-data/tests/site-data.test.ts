import { beforeEach, expect, test, vi } from "vitest";
import type { BrowsingDataResponse } from "$shared/ipc/bindings";

const native = vi.hoisted(() => ({ call: vi.fn() }));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({ browsingDataCall: native.call as never });
});
const { clearBrowsingData, clearDataForPage, forgetAllSites, forgetSite, listSites, pageSite } =
  await import("../site-data");

const profile = "00000000000000000000000001";
const kinds = { history: true, cookies: true, cache: false, downloads: true };

beforeEach(() => {
  native.call.mockReset();
});

test("a clear sends the range and exactly the chosen kinds for the given profile", async () => {
  native.call.mockResolvedValue({ kind: "cleared" } satisfies BrowsingDataResponse);
  expect(await clearBrowsingData(profile, "four_weeks", kinds)).toBe("cleared");
  expect(native.call).toHaveBeenCalledExactlyOnceWith(profile, {
    kind: "clear",
    range: "four_weeks",
    ...kinds,
  });
});

test("an engine that did not finish, a refusal and a lost call are all failures", async () => {
  for (const response of [
    { kind: "error", error: "failed" },
    { kind: "error", error: "unavailable" },
    { kind: "error", error: "invalid" },
    { kind: "sites", sites: [], partial: false },
  ] satisfies BrowsingDataResponse[]) {
    native.call.mockResolvedValueOnce(response);
    expect(await clearBrowsingData(profile, "hour", kinds)).toBe("failed");
  }
  native.call.mockRejectedValueOnce(new Error("offline"));
  expect(await clearBrowsingData(profile, "hour", kinds)).toBe("failed");
});

test("a private window says it has nothing to clear", async () => {
  native.call.mockResolvedValue({ kind: "error", error: "private" });
  expect(await clearBrowsingData(profile, "hour", kinds)).toBe("private");
});

test("the site list is the engine's sites, or null when it could not be listed", async () => {
  const sites = [{ site: "example.com", cookies: true, storage: false }];
  native.call.mockResolvedValueOnce({ kind: "sites", sites, partial: true });
  expect(await listSites(profile)).toEqual({ sites, partial: true });
  expect(native.call).toHaveBeenLastCalledWith(profile, { kind: "sites" });
  native.call.mockResolvedValueOnce({ kind: "error", error: "unavailable" });
  expect(await listSites(profile)).toBeNull();
  native.call.mockRejectedValueOnce(new Error("offline"));
  expect(await listSites(profile)).toBeNull();
});

test("removing one site names only that site, and removing all clears cookies and site data of all time", async () => {
  native.call.mockResolvedValue({ kind: "cleared" });
  expect(await forgetSite(profile, "example.com")).toBe("cleared");
  expect(native.call).toHaveBeenLastCalledWith(profile, {
    kind: "forget_site",
    site: "example.com",
  });
  expect(await forgetAllSites(profile)).toBe("cleared");
  expect(native.call).toHaveBeenLastCalledWith(profile, {
    kind: "clear",
    range: "everything",
    history: false,
    cookies: true,
    cache: false,
    downloads: false,
  });
});

test("a page's site is cleared by its origin, never its address", async () => {
  native.call.mockResolvedValue({ kind: "cleared" });
  expect(await clearDataForPage(profile, "https://news.example.com:8443/a?b=c#d")).toBe("cleared");
  expect(native.call).toHaveBeenCalledExactlyOnceWith(profile, {
    kind: "forget_page",
    origin: "https://news.example.com:8443",
  });
});

test("a page's site is its origin and host", () => {
  expect(pageSite("https://news.example.com:8443/a?b=c#d")).toEqual({
    origin: "https://news.example.com:8443",
    host: "news.example.com",
  });
});

test("an address that is not a web page has no site data to clear", async () => {
  for (const url of ["about:blank", "file:///tmp/a.html", "zephium://settings", "nonsense", ""]) {
    expect(pageSite(url)).toBeNull();
    expect(await clearDataForPage(profile, url)).toBe("failed");
  }
  expect(pageSite(null)).toBeNull();
  expect(native.call).not.toHaveBeenCalled();
});
