import { afterEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page } from "vitest/browser";
import type { TabFailure } from "$shared/ipc/bindings";
import { tabFixture } from "$shared/testing/fixtures";
import NavigationError from "../components/NavigationError.svelte";

const continued = vi.hoisted(() => vi.fn(async () => ({ outcome: "accepted" })));

vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({ tabsContinuePastCertificate: continued as never });
});

afterEach(() => continued.mockClear());

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
