import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { page } from "vitest/browser";
import { render } from "vitest-browser-svelte";
import "$styles/global.css";
import { accountSession as session } from "$domain/account";
import type { AccountV1 } from "$shared/ipc/bindings";
import { accountFixture } from "$shared/testing/account";
import AccountDelete from "../components/account/AccountDelete.svelte";

const native = vi.hoisted(() => ({
  account: null as AccountV1 | null,
  send: vi.fn(),
  remove: vi.fn(),
}));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  const view = async () => ({ account: native.account!, fault: null, retry_after: null });
  return mockBindings({
    accountGet: view,
    accountRefresh: view,
    accountDeletionCode: native.send,
    accountDelete: native.remove,
  });
});

beforeEach(async () => {
  native.account = accountFixture({ signed_in: true, email: "ada@example.com" });
  native.send.mockReset().mockImplementation(async () => ({
    account: native.account,
    fault: null,
    retry_after: null,
  }));
  await session.retain();
});
afterEach(async () => {
  native.account = accountFixture();
  await session.refresh();
  session.release();
});

test("the emailed deletion code can be entered after leaving and returning to Account", async () => {
  await page.viewport(900, 800);
  const screen = await render(AccountDelete);
  await screen.getByRole("button", { name: "Delete…", exact: true }).click();
  await page.getByRole("button", { name: "Email me a code" }).click();
  await expect.element(page.getByLabelText("Code from the email")).toBeVisible();
  await page.getByRole("button", { name: "Cancel", exact: true }).click();
  await screen.getByRole("button", { name: "Delete…", exact: true }).click();
  await expect.element(page.getByLabelText("Code from the email")).toBeVisible();
  await page.getByLabelText("Code from the email").fill("123456");
  await screen.unmount();

  const again = await render(AccountDelete);
  await again.getByRole("button", { name: "Delete…", exact: true }).click();
  await expect.element(page.getByLabelText("Code from the email")).toHaveValue("");
  await page.screenshot({ path: "../../../../../target/account-deletion-qa/after-remount.png" });
  expect(native.send).toHaveBeenCalledTimes(1);
  await page.getByLabelText("Code from the email").fill("123456");
  native.remove.mockImplementation(async () => {
    native.account = accountFixture();
    return { account: native.account, fault: null, retry_after: null };
  });
  await page.getByRole("button", { name: "Delete account", exact: true }).click();
  await expect.poll(() => native.remove.mock.calls).toEqual([["123456"]]);
  await expect.element(page.getByRole("dialog")).not.toBeInTheDocument();
});

test.each(["signed_out", "email", "server"])(
  "a pending deletion step is cleared after an account %s change",
  async (change) => {
    await session.sendDeletionCode();
    expect(session.awaitingDeletionCode()).toBe(true);
    native.account = {
      ...native.account!,
      ...(change === "signed_out"
        ? { signed_in: false }
        : change === "email"
          ? { email: "grace@example.com" }
          : { server: "https://accounts.example.com" }),
    };
    await session.refresh();
    expect(session.awaitingDeletionCode()).toBe(false);
  },
);

test("a failed send does not advance the deletion form", async () => {
  native.send.mockResolvedValue({ account: native.account, fault: "server", retry_after: null });
  expect(await session.sendDeletionCode()).toBe(false);
  expect(session.awaitingDeletionCode()).toBe(false);
});

test("an unrecognized account response does not retain a pending deletion step", async () => {
  native.send.mockResolvedValue({
    account: { ...native.account, version: 2 },
    fault: null,
    retry_after: null,
  });
  await session.sendDeletionCode();
  expect(session.awaitingDeletionCode()).toBe(false);
});

test("requesting a replacement code is explicit and clears the typed digits", async () => {
  await session.sendDeletionCode();
  const screen = await render(AccountDelete);
  await screen.getByRole("button", { name: "Delete…", exact: true }).click();
  await page.getByLabelText("Code from the email").fill("123456");
  await page.getByRole("button", { name: "Send a new code", exact: true }).click();
  await expect.element(page.getByLabelText("Code from the email")).toHaveValue("");
  expect(native.send).toHaveBeenCalledTimes(2);
});
