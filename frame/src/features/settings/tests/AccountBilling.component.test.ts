import "$styles/global.css";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page } from "vitest/browser";
import type {
  AccountPlanChangeV1,
  AccountReplyV1,
  AccountV1,
  AccountFaultV1,
} from "$shared/ipc/bindings";
import { accountFixture, usageFixture } from "$shared/testing/account";
import AccountPage from "../components/sections/AccountPage.svelte";
import { ask } from "../lib/settings-state.svelte";

const native = vi.hoisted(() => ({
  get: vi.fn(),
  buy: vi.fn(),
  manage: vi.fn(),
  limit: vi.fn(),
  preview: vi.fn(),
  confirm: vi.fn(),
  dismiss: vi.fn(),
  reauthCode: vi.fn(),
  verify: vi.fn(),
  refresh: vi.fn(),
}));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    chromeModal: async () => {},
    accountGet: native.get,
    accountRefresh: native.refresh,
    accountBuy: native.buy,
    accountManage: native.manage,
    accountSetDailyLimit: native.limit,
    accountPlanPreview: native.preview,
    accountPlanConfirm: native.confirm,
    accountPlanDismiss: native.dismiss,
    accountReauthCode: native.reauthCode,
    accountVerify: native.verify,
    profilesGet: async () => ({ status: "ok" as const, data: null }),
    settingGet: async () => null,
  });
});

const reply = (value: AccountV1, fault: AccountFaultV1 | null = null): AccountReplyV1 => ({
  account: value,
  fault,
  retry_after: null,
});

const subscription = (status: string, plan: string | null = "pro") => ({
  status,
  plan: status === "active" ? plan : null,
  interval: status === "active" ? "monthly" : null,
  paid_until: "2026-11-19T00:00:00Z",
  cancel_at_period_end: false,
});

const offers: AccountV1["offers"] = [
  {
    kind: "plan",
    plan: "pro",
    interval: "monthly",
    name: "Pro",
    price: { amount_minor: 2000, currency: "USD" },
    features: [],
  },
  {
    kind: "plan",
    plan: "max",
    interval: "monthly",
    name: "Max",
    price: { amount_minor: 10000, currency: "USD" },
    features: [],
  },
  {
    kind: "top_up",
    id: "staging-topup",
    name: "Credits",
    units: "1000000",
    price: { amount_minor: 100, currency: "USD" },
    expires_days: 365,
  },
];

const free = accountFixture({
  signed_in: true,
  email: "ada@example.com",
  plan: "free",
  usage: usageFixture({
    used_percent: 6,
    allowance_total: "2000000",
    allowance_used: "120000",
    allowance_left: "1880000",
    resets_at: null,
    one_time: true,
  }),
  subscription: subscription("inactive"),
  offers,
});

const pro = accountFixture({
  ...free,
  plan: "pro",
  usage: usageFixture({
    used_percent: 10,
    allowance_total: "10000000",
    allowance_used: "1000000",
    allowance_left: "9000000",
    resets_at: "2026-11-19T00:00:00Z",
  }),
  subscription: subscription("active"),
});

const quote = (over: Partial<AccountPlanChangeV1> = {}): AccountPlanChangeV1 => ({
  id: "7d3c1f0e-5a42-4c8e-9b61-0f2d8a4e6b13",
  status: "quoted",
  current_plan: "pro",
  target_plan: "max",
  interval: "monthly",
  direction: "upgrade",
  effective_at: null,
  charge: { amount_minor: 6500, currency: "USD" },
  recurring: { amount_minor: 10000, currency: "USD" },
  expires_at: new Date(Date.now() + 5 * 60_000).toISOString(),
  error: null,
  ...over,
});

beforeEach(() => {
  vi.clearAllMocks();
  native.refresh.mockImplementation(async () => native.get());
});
afterEach(() => {
  vi.clearAllMocks();
});

test("a one-time trial says it never renews, and shows credits only from the server's factor", async () => {
  await page.viewport(1100, 1400);
  native.get.mockResolvedValue(reply(free));
  const screen = await render(AccountPage);
  await expect.element(screen.getByText("188 of 200 credits left")).toBeVisible();
  await expect
    .element(screen.getByText("One-time free credits. They don’t renew.", { exact: false }))
    .toBeVisible();
  expect(screen.getByText(/refill/u).elements()).toHaveLength(0);
  await screen.unmount();

  // A server that states no factor has percentages, never credit numbers.
  native.get.mockResolvedValue(
    reply({
      ...free,
      usage: usageFixture({ ...free.usage!, credit_units: null, allowance_left: "1880000" }),
    }),
  );
  const older = await render(AccountPage);
  await expect.element(older.getByText("6% used")).toBeVisible();
  expect(older.getByText(/credits left/u).elements()).toHaveLength(0);
  expect(older.getByText("Daily limit").elements()).toHaveLength(0);
});

test("a plan says when its credits refill, and paused free usage says purchased credits still work", async () => {
  await page.viewport(1100, 1400);
  native.get.mockResolvedValue(
    reply({
      ...pro,
      usage: usageFixture({
        ...pro.usage!,
        free_paused: true,
        purchased_usable: true,
        purchased_remaining: "3400000",
      }),
    }),
  );
  const screen = await render(AccountPage);
  await expect.element(screen.getByText(/^Credits refill Nov 1[89]/u)).toBeVisible();
  await expect
    .element(screen.getByText("Free usage is paused right now; your purchased credits still work."))
    .toBeVisible();
  await expect.element(screen.getByText("340 credits left")).toBeVisible();
});

test("a paid account sees the way to the other plan, and a quote shows its price, charge and timing", async () => {
  await page.viewport(1100, 1600);
  native.get.mockResolvedValue(reply(pro));
  native.preview.mockResolvedValue(reply({ ...pro, plan_change: quote() }));
  const screen = await render(AccountPage);

  expect(screen.getByRole("button", { name: "Upgrade" }).elements()).toHaveLength(0);
  await screen.getByRole("button", { name: "Change plan" }).click();
  expect(screen.getByRole("button", { name: /^Upgrade to/u }).elements()).toHaveLength(0);
  await screen.getByRole("button", { name: "Switch to Max" }).click();
  expect(native.preview).toHaveBeenCalledWith("max", "monthly");
  await expect.element(screen.getByRole("heading", { name: "Upgrade to Max" })).toBeVisible();
  await expect.element(screen.getByText("$100 a month")).toBeVisible();
  await expect.element(screen.getByText("$65")).toBeVisible();
  await expect.element(screen.getByText("Due today (estimate)")).toBeVisible();
  // An estimate with no tax worked out here: Polar decides the final total.
  await expect.element(screen.getByText("An estimate.", { exact: false })).toBeVisible();
  await expect.element(screen.getByText("Right away")).toBeVisible();
});

test("confirming needs a fresh sign-in; the same quote then goes through and is followed to the end", async () => {
  await page.viewport(1100, 1600);
  const held = { ...pro, plan_change: quote() };
  native.get.mockResolvedValue(reply(held));
  native.confirm
    .mockResolvedValueOnce(reply(held, "reauth_required"))
    .mockResolvedValueOnce(reply({ ...pro, plan_change: quote({ status: "queued" }) }));
  native.reauthCode.mockResolvedValue(reply({ ...held, pending_email: "ada@example.com" }));
  native.verify.mockResolvedValue(reply(held));
  const screen = await render(AccountPage);

  await screen.getByRole("button", { name: "Confirm and pay" }).click();
  await expect
    .element(screen.getByText("For your security, confirm it’s you first.", { exact: false }))
    .toBeVisible();
  await screen.getByRole("button", { name: "Email me a sign-in code" }).click();
  await screen.getByLabelText("Sign-in code").fill("123456");
  await screen.getByRole("button", { name: "Confirm" }).click();
  expect(native.verify).toHaveBeenCalledWith("123456");
  expect(native.confirm).toHaveBeenCalledTimes(2);
  await expect.element(screen.getByText("Changing your plan…")).toBeVisible();
});

test("an unconfirmed payment is waited on, and a finished change is put away with Done", async () => {
  await page.viewport(1100, 1600);
  native.get.mockResolvedValue(reply({ ...pro, plan_change: quote({ status: "uncertain" }) }));
  const waiting = await render(AccountPage);
  await expect.element(waiting.getByText("Confirming your payment…")).toBeVisible();
  await expect
    .element(waiting.getByText("you won’t be charged twice", { exact: false }))
    .toBeVisible();
  expect(waiting.getByRole("button", { name: "Confirm and pay" }).elements()).toHaveLength(0);
  expect(waiting.getByRole("button", { name: "Switch to Max" }).elements()).toHaveLength(0);
  await waiting.unmount();

  native.get.mockResolvedValue(
    reply({
      ...pro,
      plan: "max",
      plan_change: quote({
        status: "scheduled",
        direction: "downgrade",
        current_plan: "max",
        target_plan: "pro",
        effective_at: "2026-11-19T00:00:00Z",
      }),
    }),
  );
  native.dismiss.mockResolvedValue(reply({ ...pro, plan: "max" }));
  const done = await render(AccountPage);
  await expect.element(done.getByText(/^Your plan changes to Pro on Nov 1[89]/u)).toBeVisible();
  await expect.element(done.getByText("You keep Max until then.")).toBeVisible();
  await done.getByRole("button", { name: "Done" }).click();
  expect(native.dismiss).toHaveBeenCalledTimes(1);
});

test("a failed change names why and leaves the plan alone", async () => {
  await page.viewport(1100, 1600);
  native.get.mockResolvedValue(
    reply({ ...pro, plan_change: quote({ status: "failed", error: "payment_failed" }) }),
  );
  const screen = await render(AccountPage);
  await expect.element(screen.getByText("The change didn’t go through")).toBeVisible();
  await expect
    .element(
      screen.getByText("The payment didn’t go through, so nothing changed.", { exact: false }),
    )
    .toBeVisible();
});

test("an expired quote asks for a new one rather than confirming", async () => {
  await page.viewport(1100, 1600);
  native.get.mockResolvedValue(
    reply({
      ...pro,
      plan_change: quote({ expires_at: new Date(Date.now() - 60_000).toISOString() }),
    }),
  );
  const screen = await render(AccountPage);
  await expect.element(screen.getByText("This quote has expired.")).toBeVisible();
  expect(screen.getByRole("button", { name: "Confirm and pay" }).elements()).toHaveLength(0);
  await expect.element(screen.getByRole("button", { name: "Get a new quote" })).toBeVisible();
});

test("a downgrade says it waits for renewal", async () => {
  await page.viewport(1100, 1600);
  const max = { ...pro, plan: "max" };
  native.get.mockResolvedValue(
    reply({
      ...max,
      plan_change: quote({
        direction: "downgrade",
        current_plan: "max",
        target_plan: "pro",
        charge: { amount_minor: 0, currency: "USD" },
        recurring: { amount_minor: 2000, currency: "USD" },
        effective_at: "2026-11-19T00:00:00Z",
      }),
    }),
  );
  const screen = await render(AccountPage);
  await expect.element(screen.getByText(/^At renewal on Nov 1[89]/u)).toBeVisible();
  await expect.element(screen.getByText("You keep Max until then.")).toBeVisible();
  expect(screen.getByText("Due today (estimate)").elements()).toHaveLength(0);
  await expect.element(screen.getByRole("button", { name: "Confirm change" })).toBeVisible();
});

test("under billing review there are no purchase buttons, but the portal and a downgrade remain", async () => {
  await page.viewport(1100, 1600);
  const max = {
    ...pro,
    plan: "max",
    subscription: subscription("active", "max"),
    billing_review: true,
  };
  native.get.mockResolvedValue(reply(max));
  const screen = await render(AccountPage);
  await expect
    .element(screen.getByRole("heading", { name: "Your account is under billing review" }))
    .toBeVisible();
  await expect.element(screen.getByText("support@zephium.app")).toBeVisible();
  expect(screen.getByRole("button", { name: "Upgrade" }).elements()).toHaveLength(0);
  expect(screen.getByRole("button", { name: "Buy credits" }).elements()).toHaveLength(0);
  await expect.element(screen.getByRole("button", { name: "Manage subscription" })).toBeVisible();
  await screen.getByRole("button", { name: "Change plan" }).click();
  expect(screen.getByRole("button", { name: "Switch to Max" }).elements()).toHaveLength(0);
  await expect.element(screen.getByRole("button", { name: "Switch to Pro" })).toBeVisible();
  await screen.unmount();

  // A free account under review has nothing to buy at all.
  native.get.mockResolvedValue(reply({ ...free, billing_review: true }));
  const unpaid = await render(AccountPage);
  await expect.element(unpaid.getByText("Your account is under billing review")).toBeVisible();
  expect(unpaid.getByRole("button", { name: "Upgrade", exact: true }).elements()).toHaveLength(0);
  expect(unpaid.getByRole("button", { name: "Buy credits" }).elements()).toHaveLength(0);
});

test("a checkout already open is explained, and an existing subscription turns buying into switching", async () => {
  await page.viewport(1100, 1600);
  native.get.mockResolvedValue(reply(free));
  native.buy.mockResolvedValueOnce(reply(free, "checkout_in_progress"));
  const screen = await render(AccountPage);
  await screen.getByRole("button", { name: "Upgrade", exact: true }).click();
  await screen.getByRole("button", { name: "Upgrade to Pro" }).click();
  await expect
    .element(screen.getByText("A checkout is already open.", { exact: false }))
    .toBeVisible();

  native.buy.mockResolvedValueOnce(reply(free, "subscription_change_required"));
  native.refresh.mockImplementation(async () => reply(pro));
  await screen.getByRole("button", { name: "Upgrade", exact: true }).click();
  await screen.getByRole("button", { name: "Upgrade to Pro" }).click();
  await screen.getByRole("button", { name: "Change plan" }).click();
  await expect.element(screen.getByRole("button", { name: "Switch to Max" })).toBeVisible();
});

test("credits are bought from a dialog of packs, the chosen one named on the button", async () => {
  await page.viewport(1100, 1600);
  native.get.mockResolvedValue(reply(free));
  native.buy.mockResolvedValue(reply(free));
  const screen = await render(AccountPage);
  await screen.getByRole("button", { name: "Buy credits" }).click();
  await expect.element(screen.getByText("Lasts 365 days")).toBeVisible();
  await expect.element(screen.getByRole("radio", { name: /100/u })).toBeChecked();
  await screen.getByRole("button", { name: "Buy 100 credits · $1" }).click();
  expect(native.buy).toHaveBeenCalledWith({ kind: "top_up", id: "staging-topup" });
  await expect.element(screen.getByRole("dialog")).not.toBeInTheDocument();
});

test("Buy credits chosen elsewhere opens the packs as the account shows", async () => {
  await page.viewport(1100, 1600);
  native.get.mockResolvedValue(reply(free));
  ask("credits");
  const screen = await render(AccountPage);
  await expect.element(screen.getByRole("dialog")).toBeVisible();
  await expect.element(screen.getByText("Lasts 365 days")).toBeVisible();
});

test("several packs are a choice of radios, and the first is chosen", async () => {
  await page.viewport(1100, 1600);
  native.get.mockResolvedValue(
    reply(
      accountFixture({
        ...free,
        offers: [
          ...offers,
          {
            kind: "top_up",
            id: "big",
            name: "Credits",
            units: "5000000",
            price: { amount_minor: 500, currency: "USD" },
            expires_days: 365,
          },
        ],
      }),
    ),
  );
  const screen = await render(AccountPage);
  await screen.getByRole("button", { name: "Buy credits" }).click();
  await expect.element(screen.getByRole("radio", { name: /100/u })).toBeChecked();
  await expect.element(screen.getByRole("radio", { name: /500/u })).toBeVisible();
});

/** The packs staging sells, in no particular order, each named and priced by the server. */
const stagingPacks: AccountV1["offers"] = [
  ["credits-25", "25000000", 2500],
  ["credits-5", "5000000", 500],
  ["staging-topup", "1000000", 100],
  ["credits-50", "50000000", 5000],
  ["credits-10", "10000000", 1000],
].map(([id, units, cents]) => ({
  kind: "top_up" as const,
  id: id as string,
  name: `Zephium ${id}`,
  units: units as string,
  price: { amount_minor: cents as number, currency: "USD" },
  expires_days: 365,
}));

test("several packs are a list to pick from, smallest first: credits lead, the price follows, before tax", async () => {
  await page.viewport(1100, 1600);
  const account = accountFixture({ ...free, offers: [...offers.slice(0, 2), ...stagingPacks] });
  native.get.mockResolvedValue(reply(account));
  native.buy.mockResolvedValue(reply(account));
  const screen = await render(AccountPage);
  await screen.getByRole("button", { name: "Buy credits" }).click();
  const radios = screen.getByRole("radio").elements() as HTMLInputElement[];
  expect(radios.map((radio) => radio.value)).toEqual([
    "staging-topup",
    "credits-5",
    "credits-10",
    "credits-25",
    "credits-50",
  ]);
  const rows = radios.map((radio) =>
    radio.closest("label")!.textContent!.replace(/\s+/gu, " ").trim(),
  );
  expect(rows[2]).toBe("1,000credits $10");
  // Every pack lasts as long, so it is said once rather than on each row.
  await expect.element(screen.getByText("Lasts 365 days")).toBeVisible();
  expect(screen.getByText("Lasts 365 days").elements()).toHaveLength(1);
  await expect
    .element(screen.getByText("Prices exclude tax, which is added at checkout."))
    .toBeVisible();
  await screen.getByText("$25", { exact: true }).click();
  await screen.getByRole("button", { name: "Buy 2,500 credits · $25" }).click();
  expect(native.buy).toHaveBeenCalledWith({ kind: "top_up", id: "credits-25" });
});

test("prices in the plans dialog are before tax, and an upgrade's charge is only an estimate", async () => {
  await page.viewport(1100, 1600);
  native.get.mockResolvedValue(reply(free));
  const screen = await render(AccountPage);
  await screen.getByRole("button", { name: "Upgrade", exact: true }).click();
  await expect
    .element(screen.getByText("Prices exclude tax, which is added at checkout."))
    .toBeVisible();
  screen.unmount();

  native.get.mockResolvedValue(reply(accountFixture({ ...pro, plan_change: quote() })));
  const quoted = await render(AccountPage);
  await expect.element(quoted.getByText("Due today (estimate)")).toBeVisible();
  await expect
    .element(
      quoted.getByText("An estimate. Polar works out the final total, with any tax", {
        exact: false,
      }),
    )
    .toBeVisible();
  expect(quoted.getByText("Before tax", { exact: false }).elements()).toHaveLength(0);
});

test("the plans dialog marks the plan and interval the subscription is for, not the access plan", async () => {
  await page.viewport(1100, 1600);
  native.get.mockResolvedValue(
    reply(accountFixture({ ...pro, subscription: subscription("active", "max") })),
  );
  const screen = await render(AccountPage);
  await screen.getByRole("button", { name: "Change plan" }).click();
  await expect.element(screen.getByText("Current plan · Monthly")).toBeVisible();
  await expect.element(screen.getByRole("button", { name: "Switch to Pro" })).toBeVisible();
  expect(screen.getByRole("button", { name: "Switch to Max" }).elements()).toHaveLength(0);
});

test("the plans dialog marks the current plan, offers each other plan, and closes with Escape", async () => {
  await page.viewport(1100, 1600);
  native.get.mockResolvedValue(reply(free));
  const screen = await render(AccountPage);
  const { userEvent } = await import("vitest/browser");
  // Opened from the keyboard, since WebKit does not focus a button a mouse presses.
  screen.getByRole("button", { name: "Upgrade", exact: true }).element().focus();
  await userEvent.keyboard("{Enter}");
  await expect.element(screen.getByRole("dialog", { name: "Plans" })).toBeVisible();
  await expect.element(screen.getByText("Current plan")).toBeVisible();
  await expect.element(screen.getByRole("button", { name: "Upgrade to Pro" })).toBeVisible();
  await expect.element(screen.getByRole("button", { name: "Upgrade to Max" })).toBeVisible();
  // Monthly alone is on sale here, so there is nothing to toggle or to call a saving.
  expect(screen.getByRole("radio", { name: "Yearly" }).elements()).toHaveLength(0);
  await userEvent.keyboard("{Escape}");
  await expect.element(screen.getByRole("dialog")).not.toBeInTheDocument();
  await expect.element(screen.getByRole("button", { name: "Upgrade", exact: true })).toHaveFocus();
});

test("the portal waits quietly for changes made there", async () => {
  await page.viewport(1100, 1600);
  native.get.mockResolvedValue(reply(pro));
  native.manage.mockResolvedValue(reply({ ...pro, checkout_open: true, portal_open: true }));
  const screen = await render(AccountPage);
  await screen.getByRole("button", { name: "Manage subscription" }).click();
  await expect
    .element(screen.getByText("Make your changes in the new tab.", { exact: false }))
    .toBeVisible();
});

test("a daily limit is set in whole credits, sent as units, and removed", async () => {
  await page.viewport(1100, 1600);
  native.get.mockResolvedValue(reply(free));
  native.limit.mockResolvedValueOnce(
    reply({ ...free, usage: usageFixture({ ...free.usage!, daily_limit: "1000000" }) }),
  );
  const screen = await render(AccountPage);
  await expect.element(screen.getByText("No limit")).toBeVisible();
  await screen.getByRole("button", { name: "Set…" }).click();
  const field = screen.getByLabelText("Credits per day");
  await field.fill("4.5");
  await expect.element(screen.getByRole("button", { name: "Save" })).toBeDisabled();
  await field.fill("100");
  await screen.getByRole("button", { name: "Save" }).click();
  expect(native.limit).toHaveBeenLastCalledWith("1000000");
  await expect
    .element(screen.getByText("84 of 100 credits used today", { exact: false }))
    .toBeVisible();

  native.limit.mockResolvedValueOnce(reply(free));
  await screen.getByRole("button", { name: "Change…" }).first().click();
  await screen.getByRole("button", { name: "Remove limit" }).click();
  expect(native.limit).toHaveBeenLastCalledWith(null);
});

test("a limit above the plan's cap is refused with a sentence, not a code", async () => {
  await page.viewport(1100, 1600);
  native.get.mockResolvedValue(reply(free));
  native.limit.mockResolvedValue(reply(free, "invalid"));
  const screen = await render(AccountPage);
  await screen.getByRole("button", { name: "Set…" }).click();
  await screen.getByLabelText("Credits per day").fill("9999");
  await screen.getByRole("button", { name: "Save" }).click();
  await expect
    .element(screen.getByText("That limit isn’t allowed.", { exact: false }))
    .toBeVisible();
});

test("an invitation-only server says so before anyone waits for a code", async () => {
  await page.viewport(1100, 1200);
  native.get.mockResolvedValue(reply(accountFixture({ signup_mode: "allowlist" })));
  const screen = await render(AccountPage);
  await expect
    .element(screen.getByText("Accounts are by invitation right now.", { exact: false }))
    .toBeVisible();
  await screen.unmount();

  native.get.mockResolvedValue(reply(accountFixture({ signup_mode: "closed" })));
  const closed = await render(AccountPage);
  await expect
    .element(closed.getByText("This server isn’t taking new accounts.", { exact: false }))
    .toBeVisible();
});
