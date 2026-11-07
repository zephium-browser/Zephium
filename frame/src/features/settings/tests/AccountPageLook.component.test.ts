import "$styles/global.css";
import { test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page } from "vitest/browser";
import type {
  AccountHistoryV1,
  AccountRequestV1,
  AccountSessionV1,
  AccountV1,
} from "$shared/ipc/bindings";
import {
  accountFixture,
  landscapeFixture,
  portraitFixture,
  usageFixture,
} from "$shared/testing/account";
import AccountPageStage from "./AccountPageStage.svelte";

const state = vi.hoisted(() => ({
  account: null as AccountV1 | null,
  history: null as AccountHistoryV1 | null,
}));
// Rust serves the account picture and a pick being framed; pictures stand in here.
vi.mock("$domain/account/account-model", async (original) => ({
  ...(await original<typeof import("$domain/account/account-model")>()),
  avatarUrl: (revision: number | null | undefined) => (revision ? portraitFixture : null),
  stagedAvatarUrl: () => landscapeFixture,
}));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    chromeModal: async () => {},
    accountGet: async () => ({ account: state.account!, fault: null, retry_after: null }),
    accountUsageHistory: async () => state.history ?? history,
    profilesGet: async () => ({ status: "ok" as const, data: null }),
    accountReceipt: async (id: string) => ({
      request: [...history.requests, ...days.requests].find((row) => row.id === id) ?? null,
      fault: null,
    }),
    accountSessions: async () => ({ sessions: devices, fault: null }),
    accountAvatarPick: async () => ({
      status: "ok" as const,
      data: {
        kind: "picked" as const,
        token: "01JABCDEFGHJKMNPQRSTVWXYZ0",
        width: 1600,
        height: 900,
      },
    }),
    accountAvatarDiscard: async () => ({ account: state.account!, fault: null, retry_after: null }),
    workCall: vi.fn(async (profile: string, call: unknown) => {
      const work = /"work":"(\w+)"/u.exec(JSON.stringify(call))?.[1] ?? "";
      return {
        version: 1,
        profile,
        reply: {
          kind: "projection",
          projection: { work: { objective: "" }, executions: [{ title: titles[work] ?? null }] },
        },
      } as never;
    }),
  });
});

const base = accountFixture();

function request(id: string, model: string, state: string, charged: string): AccountRequestV1 {
  return {
    id,
    model_id: model,
    state,
    charged,
    reserved: "1200000",
    started_at: "2026-10-09T10:0" + id + ":00Z",
    finished_at: state === "running" ? null : "2026-10-09T10:0" + id + ":20Z",
    error_class: null,
    funding: state === "settled" ? "allowance" : null,
    source: id === "3" ? null : { kind: "task", profile: "p1", work: "w1" },
  };
}

const history: AccountHistoryV1 = {
  requests: [
    request("5", "fast", "running", "0"),
    request("4", "fast", "settled", "312000"),
    request("3", "deep", "settled", "1840000"),
    request("2", "fast", "usage_unknown", "0"),
    request("1", "fast", "released", "0"),
  ],
  next_cursor: "more",
  fault: null,
};

const titles: Record<string, string> = {
  w1: "Standing desks, compared",
  w2: "Flights to Lisbon in March",
  w3: "Summarise the Q3 board deck",
  w4: "Reply to the landlord about the lease",
};

/** Calls spread over the last days, newest first, the way the service lists them. */
const days: AccountHistoryV1 = (() => {
  const now = Date.now();
  const midnight = new Date(now).setHours(0, 0, 0, 0);
  const at = (time: number) => new Date(time).toISOString();
  const minutes = 60_000;
  const hours = 60 * minutes;
  const call = (
    id: string,
    start: number,
    work: string | null,
    charged: string,
    model = "fast",
    callState = "settled",
  ): AccountRequestV1 => ({
    id,
    model_id: model,
    state: callState,
    charged,
    reserved: "1200000",
    started_at: at(start),
    finished_at: callState === "running" ? null : at(start + 20_000),
    error_class: null,
    funding: callState === "settled" ? "allowance" : null,
    source:
      work === "voice" ? { kind: "voice" } : work ? { kind: "task", profile: "p1", work } : null,
  });
  return {
    requests: [
      call("d1", now - 4 * minutes, "w1", "0", "fast", "running"),
      call("d2", now - 6 * minutes, "w1", "412000"),
      call("d3", now - 9 * minutes, "w1", "1840000", "deep"),
      call("d4", now - 12 * minutes, "voice", "30000"),
      call("d5", midnight - 3 * hours, "w2", "960000"),
      call("d6", midnight - 3 * hours - 4 * minutes, "w2", "2210000", "deep"),
      call("d7", midnight - 9 * hours, "w4", "180000"),
      call("d8", midnight - 2 * 24 * hours - 5 * hours, "w3", "3120000", "deep"),
      call("d9", midnight - 2 * 24 * hours - 5 * hours - 2 * minutes, "w3", "450000"),
      call("d10", midnight - 2 * 24 * hours - 6 * hours, null, "0", "fast", "released"),
      call("d11", midnight - 11 * 24 * hours - 2 * hours, "w2", "780000"),
    ],
    next_cursor: "more",
    fault: null,
  };
})();

const devices: AccountSessionV1[] = [
  {
    id: "s1",
    device: "Zephium on macOS",
    created_at: "2026-09-02T09:00:00Z",
    last_used_at: null,
    current: true,
  },
  {
    id: "s2",
    device: "Zephium on Windows",
    created_at: "2026-09-28T18:30:00Z",
    last_used_at: null,
    current: false,
  },
  {
    id: "s3",
    device: null,
    created_at: "2026-10-12T07:45:00Z",
    last_used_at: null,
    current: false,
  },
];

const shots = "../../../../../target/account";
const settle = () => new Promise((done) => setTimeout(done, 400));

async function shoot(
  name: string,
  account: AccountV1,
  act?: (screen: Awaited<ReturnType<typeof render>>) => Promise<void>,
  height = 1600,
) {
  state.account = account;
  await page.viewport(1100, height);
  const screen = await render(AccountPageStage);
  await settle();
  await act?.(screen);
  for (const theme of ["dark", "light"]) {
    document.documentElement.dataset.theme = theme;
    await settle();
    await page.screenshot({ path: `${shots}/${name}-${theme}.png` });
  }
  document.documentElement.dataset.theme = "dark";
  await screen.unmount();
}

const signedIn: AccountV1 = {
  ...base,
  signed_in: true,
  email: "ada@example.com",
  name: "Ada Lovelace",
  plan: "free",
  usage: usageFixture({
    used_percent: 62,
    allowance_used: "1240000",
    bonus_remaining: "1500000",
    bonus_expires_at: "2026-10-20T00:00:00Z",
    purchased_remaining: "5000000",
    held: "120000",
    spent_today: "300000",
    daily_limit: "1000000",
  }),
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
      features: ["Frontier models", "More usage"],
    },
    {
      kind: "plan",
      plan: "pro",
      interval: "yearly",
      name: "Zephium Pro",
      price: { amount_minor: 19200, currency: "usd" },
      features: ["Frontier models", "More usage"],
    },
    {
      kind: "top_up",
      id: "ten",
      name: "10 credits",
      units: "10000000",
      price: { amount_minor: 1000, currency: "usd" },
      expires_days: 365,
    },
  ],
};

test("Settings → Account signed out, waiting for a code, and signed in", async () => {
  await shoot("signed-out", base);
  await shoot("code", { ...base, pending_email: "ada@example.com", resend_in: 42 });
  await shoot("signed-in", signedIn);
  await shoot("history", signedIn, async (screen) => {
    await screen.getByRole("button", { name: "Usage history" }).click();
    await settle();
    await screen
      .getByRole("button", { name: /\d calls?/u })
      .first()
      .click();
    await settle();
    await screen.getByRole("button", { name: "Details for this request" }).nth(1).click();
  });
  const pro: AccountV1 = {
    ...signedIn,
    plan: "pro",
    usage: usageFixture({
      used_percent: 10,
      allowance_total: "10000000",
      allowance_used: "1000000",
      allowance_left: "9000000",
      resets_at: "2026-11-19T00:00:00Z",
      purchased_remaining: "3400000",
      daily_limit: null,
    }),
    subscription: {
      status: "active",
      plan: null,
      interval: "monthly",
      paid_until: "2026-11-19T00:00:00Z",
      cancel_at_period_end: false,
    },
    offers: [
      ...signedIn.offers,
      {
        kind: "plan",
        plan: "max",
        interval: "monthly",
        name: "Zephium Max",
        price: { amount_minor: 10000, currency: "usd" },
        features: [],
      },
    ],
  };
  await shoot("trial", {
    ...signedIn,
    usage: usageFixture({
      used_percent: 6,
      allowance_total: "2000000",
      allowance_used: "120000",
      allowance_left: "1880000",
      resets_at: null,
      one_time: true,
      free_paused: true,
      purchased_usable: true,
      purchased_remaining: "3400000",
      bonus_remaining: null,
      held: null,
    }),
  });
  await shoot("paid", pro);
  state.history = days;
  const openHistory = (scroll: number) => async (screen: Awaited<ReturnType<typeof render>>) => {
    await screen.getByRole("button", { name: "Usage history" }).click();
    await settle();
    await screen
      .getByRole("button", { name: /Flights to Lisbon/u })
      .first()
      .click();
    await settle();
    await screen.getByRole("button", { name: "Details for this request" }).nth(1).click();
    await settle();
    document.querySelector(".settings-content")?.scrollTo({ top: scroll });
    await screen.getByText("Your Zephium AI credits by task", { exact: false }).hover();
  };
  await shoot("pro-history", pro, openHistory(240));
  // A day's heading holds at the top while its rows scroll under it.
  await shoot("pro-history-scrolled", pro, openHistory(760), 900);
  state.history = null;
  await shoot("devices", pro, async (screen) => {
    await screen.getByRole("button", { name: "Signed-in devices" }).click();
  });
  await shoot("quote", {
    ...pro,
    plan_change: {
      id: "7d3c1f0e-5a42-4c8e-9b61-0f2d8a4e6b13",
      status: "quoted",
      current_plan: "pro",
      target_plan: "max",
      interval: "monthly",
      direction: "upgrade",
      effective_at: null,
      charge: { amount_minor: 6500, currency: "usd" },
      recurring: { amount_minor: 10000, currency: "usd" },
      expires_at: new Date(Date.now() + 5 * 60_000).toISOString(),
      error: null,
    },
  });
  await shoot("review", { ...pro, billing_review: true });
  await shoot("invite-only", { ...base, signup_mode: "allowlist" });
  await shoot("email-change", signedIn, async (screen) => {
    await screen.getByRole("button", { name: "Change email…" }).click();
    await settle();
    await screen.getByLabelText("New email address").fill("ada@new.example");
  });
  const pictured = { ...signedIn, avatar: 3 };
  await shoot("avatar", pictured);
  await shoot("avatar-menu", pictured, async (screen) => {
    await screen.getByRole("button", { name: "Change photo" }).click();
  });
  await shoot("avatar-crop", signedIn, async (screen) => {
    await screen.getByRole("button", { name: "Add a photo" }).click();
    await settle();
    screen.getByRole("application").element().focus();
    const { userEvent } = await import("vitest/browser");
    await userEvent.keyboard("++++{Shift>}{ArrowLeft}{ArrowLeft}{ArrowLeft}{/Shift}");
    screen.getByRole("application").element().blur();
  });
  const packs: AccountV1["offers"] = [
    ["staging-topup", "1000000", 100],
    ["credits-5", "5000000", 500],
    ["credits-10", "10000000", 1000],
    ["credits-25", "25000000", 2500],
    ["credits-50", "50000000", 5000],
  ].map(([id, units, cents]) => ({
    kind: "top_up" as const,
    id: id as string,
    name: null,
    units: units as string,
    price: { amount_minor: cents as number, currency: "usd" },
    expires_days: 365,
  }));
  await shoot(
    "credits",
    {
      ...pictured,
      offers: [...signedIn.offers.filter((offer) => offer.kind === "plan"), ...packs],
    },
    async (screen) => {
      await screen.getByRole("button", { name: "Buy credits" }).click();
      await settle();
      await screen.getByText("$10", { exact: true }).click();
    },
  );
  await shoot(
    "plans",
    { ...pro, plan: "free", subscription: signedIn.subscription },
    async (screen) => {
      await screen.getByRole("button", { name: "Upgrade", exact: true }).click();
    },
  );
  await shoot("billing-off", {
    ...signedIn,
    billing: false,
    offers: [],
    custom_server: true,
    server: "https://zephium.example.org",
    update_required: true,
    unsettled: [history.requests[3]!],
  });
}, 180_000);
