import { afterEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page } from "vitest/browser";
import type { TabFailure } from "$shared/ipc/bindings";
import { tabFixture } from "$shared/testing/fixtures";
import NavigationError from "../components/NavigationError.svelte";

const continued = vi.hoisted(() => vi.fn(async () => ({ outcome: "accepted" })));
const relaunched = vi.hoisted(() => vi.fn(async () => true));
const left = vi.hoisted(() => vi.fn(async () => ({ accepted: true, operation_id: null })));
const unblocked = vi.hoisted(() =>
  vi.fn(async (_id: string, _url: string) => ({ accepted: true, operation_id: null })),
);

vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    tabsContinuePastCertificate: continued as never,
    appRelaunch: relaunched,
    tabsLeaveBlockedPage: left as never,
    tabsContinuePastBlock: unblocked as never,
  });
});

afterEach(() => {
  continued.mockClear();
  relaunched.mockClear();
  left.mockClear();
  unblocked.mockClear();
});

function show(failure: TabFailure) {
  const tab = tabFixture({ id: "tab-7", url: null, failure });
  return render(NavigationError, { tab, failure });
}

test("a refused certificate on a local host offers the way past behind details", async () => {
  show({
    url: "https://192.168.1.1/admin",
    reason: "insecure",
    certificate: "untrusted",
    can_continue: true,
  });
  await expect.element(page.getByRole("alert")).toHaveTextContent("Its certificate isn't trusted");
  await expect
    .element(page.getByRole("button", { name: "Continue to 192.168.1.1 (unsafe)" }))
    .not.toBeInTheDocument();

  await page.getByRole("button", { name: "Show details" }).click();
  await expect.element(page.getByRole("alert")).toHaveTextContent("Only continue if");
  await page.getByRole("button", { name: "Continue to 192.168.1.1 (unsafe)" }).click();
  expect(continued).toHaveBeenCalledExactlyOnceWith("tab-7");
});

test("a public site only says what is wrong, with no way past", async () => {
  show({
    url: "https://example.com/",
    reason: "insecure",
    certificate: "expired",
    can_continue: false,
  });
  await expect
    .element(page.getByRole("alert"))
    .toHaveTextContent("Its certificate has expired or isn't valid yet.");
  await expect.element(page.getByRole("button", { name: "Show details" })).not.toBeInTheDocument();
  await expect.element(page.getByRole("button", { name: /unsafe/ })).not.toBeInTheDocument();
});

test("a secure-connection failure without a certificate has no way past", async () => {
  show({ url: "https://nas.local/", reason: "insecure", can_continue: true });
  await expect.element(page.getByRole("button", { name: "Show details" })).not.toBeInTheDocument();
  await expect.element(page.getByRole("button", { name: "Try again" })).toBeVisible();
});

test("a profile only a restart can trust offers the restart, never the address", async () => {
  show({ url: "https://example.com/", reason: "restart_required", covers_page: true });
  await expect.element(page.getByRole("heading")).toHaveTextContent("Zephium needs to restart");
  await expect
    .element(page.getByRole("alert"))
    .toHaveTextContent("keep browsing safely in this profile");
  await expect.element(page.getByRole("button", { name: "Try again" })).not.toBeInTheDocument();
  await page.getByRole("button", { name: "Restart Zephium" }).click();
  expect(relaunched).toHaveBeenCalledOnce();
});

test("a restart that does not start says so and can be tried again", async () => {
  relaunched.mockResolvedValueOnce(false).mockRejectedValueOnce(new Error("ipc"));
  show({ url: "https://example.com/", reason: "restart_required", covers_page: true });
  const button = page.getByRole("button", { name: "Restart Zephium" });
  const failed = page.getByText("Zephium couldn't restart. Try again.");
  await button.click();
  await expect.element(failed).toBeInTheDocument();
  await button.click();
  await expect.element(failed).toBeInTheDocument();
  await button.click();
  await expect.element(failed).not.toBeInTheDocument();
  expect(relaunched).toHaveBeenCalledTimes(3);
});

test("a badware page names the list, goes back first and lets the person continue", async () => {
  show({
    url: "https://scam.example/win",
    reason: "blocked",
    blocked: "badware",
    can_continue: true,
  });
  await expect.element(page.getByRole("heading")).toHaveTextContent("Zephium blocked scam.example");
  await expect
    .element(page.getByRole("alert"))
    .toHaveTextContent("uBlock Origin's Badware list as a known scam or malware site");
  // The protection bench ends a refused load when this page shows this refusal.
  const shown = page.getByRole("alert").element() as HTMLElement;
  expect([shown.dataset.zephiumBlockedTab, shown.dataset.zephiumBlockedUrl]).toEqual([
    "tab-7",
    "https://scam.example/win",
  ]);
  await expect.element(page.getByRole("button", { name: "Try again" })).not.toBeInTheDocument();
  await page.getByRole("button", { name: "Go back" }).click();
  expect(left).toHaveBeenCalledExactlyOnceWith("tab-7");
  await page.getByRole("button", { name: "Continue anyway" }).click();
  // The address shown goes back with the choice, so only this refusal lifts.
  expect(unblocked).toHaveBeenCalledExactlyOnceWith("tab-7", "https://scam.example/win");
  expect(page.getByText("Try again.", { exact: false }).query()).toBeNull();
});

test("a refusal the engine enforces offers only Go back", async () => {
  show({
    url: "https://ad.example/",
    reason: "blocked",
    blocked: "filter_lists",
    can_continue: false,
  });
  await expect.element(page.getByRole("button", { name: "Go back" })).toBeVisible();
  expect(page.getByRole("button", { name: "Continue anyway" }).query()).toBeNull();
});

test("a choice that does not go through says so", async () => {
  left.mockResolvedValueOnce({ accepted: false, operation_id: null });
  show({ url: "https://scam.example/", reason: "blocked", blocked: "badware", can_continue: true });
  await page.getByRole("button", { name: "Go back" }).click();
  await expect.element(page.getByText("Couldn’t do that. Try again.")).toBeVisible();
});

test("a page the other lists refuse says it is an ad or tracking redirect", async () => {
  show({
    url: "https://ad.doubleclick.net/ddm/clk/1",
    reason: "blocked",
    blocked: "filter_lists",
    can_continue: false,
  });
  await expect
    .element(page.getByRole("alert"))
    .toHaveTextContent("a known ad or tracking redirect, blocked by your filter lists");
});
