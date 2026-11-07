import type { AccountUsageV1, AccountV1 } from "$shared/ipc/bindings";

/** A signed-out account on Zephium's server, as `account_get` answers it. */
export function accountFixture(overrides: Partial<AccountV1> = {}): AccountV1 {
  return {
    version: 1,
    available: true,
    server: "https://api.zephium.app",
    custom_server: false,
    hosted_models: true,
    billing: true,
    signup_open: true,
    update_required: false,
    signed_in: false,
    vault_fault: false,
    pending_email: null,
    resend_in: null,
    email: null,
    name: null,
    avatar: null,
    plan: null,
    usage: null,
    subscription: null,
    offers: [],
    checkout_open: false,
    portal_open: false,
    signup_mode: null,
    billing_review: false,
    plan_change: null,
    notice: null,
    unsettled: [],
    ...overrides,
  };
}

/**
 * Usage in integer units, a million to the server's own unit and `credit_units`
 * (10,000, as staging sends it) to a credit: 100 credits to a dollar.
 */
export function usageFixture(overrides: Partial<AccountUsageV1> = {}): AccountUsageV1 {
  return {
    used_percent: 42,
    resets_at: "2026-10-14T00:00:00Z",
    extra: false,
    unit_scale: 1_000_000,
    remaining: "1160000",
    allowance_total: "2000000",
    allowance_used: "840000",
    held: null,
    bonus_remaining: null,
    bonus_expires_at: null,
    purchased_remaining: "0",
    spent_today: "840000",
    daily_limit: null,
    credit_units: "10000",
    allowance_left: "1160000",
    one_time: false,
    free_paused: false,
    purchased_usable: null,
    ...overrides,
  };
}

const svg = (width: number, height: number, body: string) =>
  `data:image/svg+xml,${encodeURIComponent(
    `<svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="${height}" viewBox="0 0 ${width} ${height}">${body}</svg>`,
  )}`;

/** A head-and-shoulders picture standing in for an account photo in looks. */
export const portraitFixture = svg(
  256,
  256,
  `<defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#e9b38a"/><stop offset="1" stop-color="#8a5a7a"/></linearGradient></defs>
  <rect width="256" height="256" fill="url(#g)"/>
  <circle cx="128" cy="104" r="46" fill="#3b2a35" opacity="0.9"/>
  <path d="M40 256c8-58 46-88 88-88s80 30 88 88z" fill="#3b2a35" opacity="0.9"/>`,
);

/** A wide picked photo, the person off centre, for the framing step. */
export const landscapeFixture = svg(
  1600,
  900,
  `<defs><linearGradient id="s" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#9cc3e6"/><stop offset="1" stop-color="#f3d9b9"/></linearGradient></defs>
  <rect width="1600" height="900" fill="url(#s)"/>
  <path d="M0 640q300-120 620-40t560-20 420 60v260H0z" fill="#6f8f6a"/>
  <path d="M0 760q420-90 820-10t780-20v170H0z" fill="#4e6b4c"/>
  <circle cx="1040" cy="420" r="92" fill="#3b2a35"/>
  <path d="M860 900c14-170 90-260 180-260s166 90 180 260z" fill="#3b2a35"/>`,
);
