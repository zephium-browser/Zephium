import { afterEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page } from "vitest/browser";
import { clearRequest } from "$domain/site-data";
import * as notices from "$session/notice.svelte";
import ClearSiteData from "../components/ClearSiteData.svelte";

const native = vi.hoisted(() => ({ call: vi.fn(), modal: vi.fn(async (_open: boolean) => {}) }));

vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({ browsingDataCall: native.call as never, chromeModal: native.modal });
});
vi.mock("$domain/tabs", () => ({ tabs: { profile: () => ({ id: "profile" }) } }));

afterEach(() => {
  native.call.mockReset();
  native.modal.mockClear();
  clearRequest.dismiss();
});

test("nothing is shown until a command asks to clear a site's data", async () => {
  await render(ClearSiteData);
  await expect.element(page.getByRole("dialog")).not.toBeInTheDocument();
  expect(native.call).not.toHaveBeenCalled();
});

test("the dialog names the site and only clears it once confirmed", async () => {
  native.call.mockResolvedValue({ kind: "cleared" });
  await render(ClearSiteData);
  clearRequest.ask("https://www.example.com/a/b?c=d");

  const dialog = page.getByRole("dialog");
  await expect.element(dialog).toHaveTextContent("Clear data for www.example.com?");
  expect(native.call).not.toHaveBeenCalled();

  await dialog.getByRole("button", { name: "Clear data" }).click();

  expect(native.call).toHaveBeenCalledExactlyOnceWith("profile", {
    kind: "forget_page",
    origin: "https://www.example.com",
  });
  await expect.element(page.getByRole("dialog")).not.toBeInTheDocument();
  expect(notices.notice()?.text).toBe("Cleared the data of www.example.com");
});

test("cancelling clears nothing and closes the dialog", async () => {
  await render(ClearSiteData);
  clearRequest.ask("https://example.com/");

  await page.getByRole("button", { name: "Cancel" }).click();

  await expect.element(page.getByRole("dialog")).not.toBeInTheDocument();
  expect(native.call).not.toHaveBeenCalled();
  expect(clearRequest.pending()).toBeNull();
});

test("a clear that fails keeps the dialog open and names the site", async () => {
  native.call.mockResolvedValue({ kind: "error", error: "failed" });
  await render(ClearSiteData);
  clearRequest.ask("https://example.com/");

  await page.getByRole("button", { name: "Clear data" }).click();

  await expect
    .element(page.getByRole("alert"))
    .toHaveTextContent("Couldn't clear the data of example.com");
  await expect.element(page.getByRole("dialog")).toBeVisible();
  await page.getByRole("button", { name: "Cancel" }).click();
});

test("a private window says it has nothing to clear", async () => {
  native.call.mockResolvedValue({ kind: "error", error: "private" });
  await render(ClearSiteData);
  clearRequest.ask("https://example.com/");

  await page.getByRole("button", { name: "Clear data" }).click();

  await expect
    .element(page.getByRole("alert"))
    .toHaveTextContent("A private window keeps nothing to clear.");
  await page.getByRole("button", { name: "Cancel" }).click();
});

test("pages leave the screen while the dialog is open, and come back when it closes", async () => {
  await render(ClearSiteData);
  clearRequest.ask("https://www.example.com/");
  await expect.element(page.getByRole("dialog")).toBeVisible();
  await expect.poll(() => native.modal.mock.calls.at(-1)).toEqual([true]);

  await page.getByRole("button", { name: "Cancel" }).click();
  await expect.poll(() => native.modal.mock.calls.at(-1)).toEqual([false]);
  expect(native.call).not.toHaveBeenCalled();
});
