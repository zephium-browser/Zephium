import "$styles/global.css";
import { expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page, userEvent } from "vitest/browser";
import type { AccountReplyV1, AccountRequestV1, AccountV1 } from "$shared/ipc/bindings";
import { accountFixture as account, usageFixture } from "$shared/testing/account";
import { preferences } from "$domain/preferences";
import AccountPage from "../components/sections/AccountPage.svelte";

const native = vi.hoisted(() => ({
  get: vi.fn(),
  send: vi.fn(),
  verify: vi.fn(),
  cancel: vi.fn(),
  buy: vi.fn(),
  signOut: vi.fn(),
  setName: vi.fn(),
  emailCode: vi.fn(),
  reauthCode: vi.fn(),
  changeEmail: vi.fn(),
  history: vi.fn(),
  receipt: vi.fn(),
  workCall: vi.fn(),
  settings: {} as Record<string, string>,
}));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    accountGet: native.get,
    accountSendCode: native.send,
    accountVerify: native.verify,
    accountCancelCode: native.cancel,
    accountBuy: native.buy,
    accountSignOut: native.signOut,
    accountSetName: native.setName,
    accountEmailCode: native.emailCode,
    accountReauthCode: native.reauthCode,
    accountChangeEmail: native.changeEmail,
    accountUsageHistory: native.history,
    accountReceipt: native.receipt,
    workCall: native.workCall,
    profilesGet: async () => ({ status: "ok" as const, data: null }),
    settingGet: async (key: string) => native.settings[key] ?? null,
  });
});

const reply = (value: AccountV1, fault: AccountReplyV1["fault"] = null): AccountReplyV1 => ({
  account: value,
  fault,
  retry_after: null,
});

const signedIn = account({
  signed_in: true,
  email: "ada@example.com",
  plan: "free",
  usage: usageFixture(),
  subscription: {
    status: "inactive",
    plan: null,
    interval: null,
    paid_until: null,
    cancel_at_period_end: false,
  },
  offers: [
    {
      kind: "plan",
      plan: "pro",
      interval: "monthly",
      name: "Zephium Pro",
      price: { amount_minor: 2000, currency: "usd" },
      features: [],
    },
  ],
});

test("signing in takes an address, then the emailed code, with no password", async () => {
  await page.viewport(1100, 1000);
  native.get.mockResolvedValue(reply(account()));
  const waiting = account({ pending_email: "ada@example.com", resend_in: 60 });
  native.send.mockResolvedValue(reply(waiting));
  native.verify
    .mockResolvedValueOnce(reply(waiting, "invalid_code"))
    .mockResolvedValueOnce(reply(signedIn));
  const screen = await render(AccountPage);

  await expect.element(screen.getByRole("heading", { name: "Sign in to Zephium" })).toBeVisible();
  const continueButton = screen.getByRole("button", { name: "Continue" });
  await expect.element(continueButton).toBeDisabled();
  await screen.getByLabelText("Email address").fill("ada@example.com");
  await continueButton.click();
  expect(native.send).toHaveBeenCalledWith("ada@example.com");

  await expect
    .element(screen.getByText("Enter the 6-digit code sent to ada@example.com."))
    .toBeVisible();
  await expect.element(screen.getByRole("button", { name: /New code in \d+s/u })).toBeDisabled();
  await screen.getByLabelText("Code").click();
  await userEvent.keyboard("111 111");
  expect(native.verify).toHaveBeenLastCalledWith("111111");
  await expect
    .element(screen.getByText("That code didn’t work. Check it, or send a new one."))
    .toBeVisible();

  await userEvent.keyboard("123456");
  expect(native.verify).toHaveBeenLastCalledWith("123456");
  await expect.element(screen.getByRole("heading", { name: "ada@example.com" })).toBeVisible();
  await expect.element(screen.getByText("116 of 200 credits left")).toBeVisible();
  await expect.element(screen.getByText(/^Credits refill Oct 1[34]$/u)).toBeVisible();
});

test("upgrading opens checkout and says the plan will update on its own", async () => {
  await page.viewport(1100, 1200);
  native.get.mockResolvedValue(reply(signedIn));
  native.buy.mockResolvedValue(reply({ ...signedIn, checkout_open: true }));
  const screen = await render(AccountPage);

  await screen.getByRole("button", { name: "Upgrade", exact: true }).click();
  await expect.element(screen.getByText("$20")).toBeVisible();
  await screen.getByRole("button", { name: "Upgrade to Pro" }).click();
  expect(native.buy).toHaveBeenCalledWith({ kind: "plan", plan: "pro", interval: "monthly" });
  await expect
    .element(screen.getByText("Finish checkout in the new tab. Your plan updates here on its own."))
    .toBeVisible();
});

test("a server that sells nothing shows usage but no purchase", async () => {
  await page.viewport(1100, 1200);
  native.get.mockResolvedValue(reply({ ...signedIn, billing: false, offers: [] }));
  const screen = await render(AccountPage);
  await expect.element(screen.getByText("Included credits")).toBeVisible();
  expect(screen.getByRole("button", { name: "Upgrade" }).elements()).toHaveLength(0);
  expect(screen.getByRole("button", { name: "Manage subscription" }).elements()).toHaveLength(0);
});

test("welcome credit, holds and the daily limit read apart, holds never as charged", async () => {
  await page.viewport(1100, 1400);
  native.get.mockResolvedValue(
    reply({
      ...signedIn,
      usage: usageFixture({
        bonus_remaining: "1500000",
        bonus_expires_at: "2026-10-20T00:00:00Z",
        purchased_remaining: "5000000",
        held: "120000",
        spent_today: "300000",
        daily_limit: "1000000",
      }),
      unsettled: [
        {
          id: "0b3c6a3e-8f7d-4a59-9a3c-1d2e3f4a5b6c",
          model_id: "fast",
          state: "usage_unknown",
          charged: "0",
          reserved: "120000",
          started_at: "2026-10-09T10:00:00Z",
          finished_at: null,
          error_class: "client_disconnected",
          funding: null,
          source: null,
        },
      ],
    }),
  );
  const screen = await render(AccountPage);
  await expect.element(screen.getByText(/^150 left · expires Oct (19|20)$/u)).toBeVisible();
  await expect.element(screen.getByText("500 credits left")).toBeVisible();
  await expect.element(screen.getByText("12 · not charged yet")).toBeVisible();
  await expect.element(screen.getByText("30 of 100 credits used today")).toBeVisible();
  await expect
    .element(screen.getByText("A request ended before its charge was confirmed.", { exact: false }))
    .toBeVisible();
});

test("usage history loads on request, groups calls by task, pages by its cursor and opens a receipt", async () => {
  await page.viewport(1100, 1400);
  native.get.mockResolvedValue(reply(signedIn));
  const row = (
    id: string,
    state: string,
    charged: string,
    source: AccountRequestV1["source"],
  ): AccountRequestV1 => ({
    id,
    model_id: "fast",
    state,
    charged,
    reserved: "1200000",
    started_at: "2026-10-09T10:00:00Z",
    finished_at: "2026-10-09T10:00:05Z",
    error_class: null,
    funding: charged === "0" ? null : "mixed",
    source,
  });
  const task = { kind: "task" as const, profile: "p1", work: "w1" };
  native.history
    .mockResolvedValueOnce({
      requests: [row("a", "settled", "812000", task), row("b", "running", "0", task)],
      next_cursor: "opaque/+cursor",
      fault: null,
    })
    .mockResolvedValueOnce({
      requests: [
        row("c", "settled", "400000", task),
        row("d", "settled", "30000", { kind: "voice" }),
        row("e", "released", "0", null),
      ],
      next_cursor: null,
      fault: null,
    });
  native.receipt.mockResolvedValue({
    request: row("a", "settled", "812000", task),
    fault: null,
  });
  native.workCall.mockResolvedValue({
    version: 1,
    profile: "p1",
    reply: {
      kind: "projection",
      projection: {
        work: { objective: "Compare standing desks" },
        executions: [{ title: "Standing desks, compared" }],
      },
    },
  });
  const screen = await render(AccountPage);

  expect(native.history).not.toHaveBeenCalled();
  await screen.getByRole("button", { name: "Usage history" }).click();
  // One row per task: its name comes from this computer's own store, and a
  // call still running is not added to its total.
  await expect.element(screen.getByText("Standing desks, compared")).toBeVisible();
  expect(native.history).toHaveBeenLastCalledWith(null);
  await expect.element(screen.getByText("81 credits · In progress")).toBeVisible();
  expect(native.workCall).toHaveBeenCalledTimes(1);

  await screen.getByRole("button", { name: "Show more" }).click();
  expect(native.history).toHaveBeenLastCalledWith("opaque/+cursor");
  // The next page joins the same task, and voice and the rest have their own rows.
  await expect.element(screen.getByText("121 credits · In progress")).toBeVisible();
  await expect.element(screen.getByText("Voice")).toBeVisible();
  await expect.element(screen.getByText("Other", { exact: true })).toBeVisible();
  expect(screen.getByRole("button", { name: "Show more" }).elements()).toHaveLength(0);
  expect(native.workCall).toHaveBeenCalledTimes(1);

  await screen.getByRole("button", { name: /Standing desks, compared/u }).click();
  await expect.element(screen.getByText("81.2 credits")).toBeVisible();
  await expect.element(screen.getByText("In progress", { exact: true })).toBeVisible();
  await screen.getByRole("button", { name: "Details for this request" }).first().click();
  expect(native.receipt).toHaveBeenCalledWith("a");
  await expect
    .element(screen.getByText(/^Charged 81\.2 credits · Plan \+ purchased · Finished /u))
    .toBeVisible();
});

test("changing email asks for a fresh sign-in first, then signs in at the new address", async () => {
  await page.viewport(1100, 1200);
  native.get.mockResolvedValue(reply(signedIn));
  native.emailCode
    .mockResolvedValueOnce(reply(signedIn, "reauth_required"))
    .mockResolvedValueOnce(reply(signedIn));
  native.reauthCode.mockResolvedValue(reply({ ...signedIn, pending_email: "ada@example.com" }));
  native.verify.mockResolvedValue(reply(signedIn));
  const moved = account({ pending_email: "ada@new.example", resend_in: 60 });
  native.changeEmail.mockResolvedValue(reply(account()));
  native.send.mockResolvedValue(reply(moved));
  const screen = await render(AccountPage);

  await screen.getByRole("button", { name: "Change email…" }).click();
  await screen.getByLabelText("New email address").fill("ada@new.example");
  await screen.getByRole("button", { name: "Send code" }).click();
  expect(native.emailCode).toHaveBeenLastCalledWith("ada@new.example");
  await expect
    .element(screen.getByText("For your security, confirm it’s you first.", { exact: false }))
    .toBeVisible();

  await screen.getByRole("button", { name: "Email me a sign-in code" }).click();
  await screen.getByLabelText("Sign-in code").fill("123456");
  await screen.getByRole("button", { name: "Confirm" }).click();
  expect(native.verify).toHaveBeenLastCalledWith("123456");
  expect(native.emailCode).toHaveBeenCalledTimes(2);

  await screen.getByLabelText("Code from the email").fill("654321");
  await screen.getByRole("button", { name: "Change email", exact: true }).click();
  expect(native.changeEmail).toHaveBeenCalledWith("ada@new.example", "654321");
  expect(native.send).toHaveBeenLastCalledWith("ada@new.example");
  await expect
    .element(screen.getByText("Your email is now ada@new.example.", { exact: false }))
    .toBeVisible();
});

test("the name can be set and cleared", async () => {
  await page.viewport(1100, 1200);
  native.get.mockResolvedValue(reply(signedIn));
  native.setName.mockResolvedValue(reply({ ...signedIn, name: "Ada Lovelace" }));
  const screen = await render(AccountPage);
  await screen.getByRole("button", { name: "Edit name" }).click();
  await screen.getByLabelText("Name").fill("  Ada Lovelace ");
  await screen.getByRole("button", { name: "Save" }).click();
  expect(native.setName).toHaveBeenCalledWith("Ada Lovelace");
  await expect.element(screen.getByRole("heading", { name: "Ada Lovelace" })).toBeVisible();
});

test("signing out returns to the sign-in screen", async () => {
  await page.viewport(1100, 1200);
  native.get.mockResolvedValue(reply(signedIn));
  native.signOut.mockResolvedValue(reply(account()));
  const screen = await render(AccountPage);
  await screen.getByRole("button", { name: "Sign out", exact: true }).first().click();
  await expect.element(screen.getByLabelText("Email address")).toBeVisible();
});

test("a sign-in the Keychain will not give up is said plainly, with a way to sign in again", async () => {
  await page.viewport(1100, 1200);
  native.get.mockResolvedValue(reply({ ...signedIn, vault_fault: true }));
  native.signOut.mockClear();
  native.signOut.mockResolvedValue(reply(account()));
  const screen = await render(AccountPage);
  await expect
    .element(screen.getByText("can’t read your sign-in from the Keychain", { exact: false }))
    .toBeVisible();
  expect(native.signOut).not.toHaveBeenCalled();
  await screen.getByRole("button", { name: "Sign in again" }).click();
  expect(native.signOut).toHaveBeenCalledTimes(1);
  await expect.element(screen.getByLabelText("Email address")).toBeVisible();
});

test("a readable sign-in shows no Keychain warning", async () => {
  await page.viewport(1100, 1200);
  native.get.mockResolvedValue(reply(signedIn));
  const screen = await render(AccountPage);
  await expect
    .element(screen.getByRole("button", { name: "Sign out", exact: true }).first())
    .toBeVisible();
  expect(screen.getByText("can’t read your sign-in", { exact: false }).elements()).toHaveLength(0);
});

test("with Work and AI off the plan stays and Zephium AI's usage and credits go", async () => {
  await page.viewport(1100, 1200);
  native.settings = { "ai.enabled": "false" };
  await preferences.init();
  native.get.mockResolvedValue(reply(signedIn));
  const screen = await render(AccountPage);
  await expect.element(screen.getByText("Plan", { exact: true })).toBeVisible();
  await expect.element(screen.getByRole("button", { name: "Upgrade" })).toBeVisible();
  expect(screen.getByText("Included credits").elements()).toHaveLength(0);
  expect(screen.getByText("Purchased credits").elements()).toHaveLength(0);
  expect(screen.getByText("Zephium AI").elements()).toHaveLength(0);
  screen.unmount();
  preferences.dispose();
  native.settings = {};
});
