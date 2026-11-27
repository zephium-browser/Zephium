import "$styles/global.css";
import { test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page, userEvent } from "vitest/browser";
import type { AccountReplyV1, AccountV1 } from "$shared/ipc/bindings";
import { accountFixture, usageFixture } from "$shared/testing/account";
import AccountPageStage from "./AccountPageStage.svelte";

const native = vi.hoisted(() => ({
  get: vi.fn(),
  send: vi.fn(),
  verify: vi.fn(),
  cancel: vi.fn(),
}));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    accountGet: native.get,
    accountSendCode: native.send,
    accountVerify: native.verify,
    accountCancelCode: native.cancel,
    accountSessions: async () => ({ sessions: [], fault: null }),
    accountUsageHistory: async () => ({ requests: [], next_cursor: null, fault: null }),
    profilesGet: async () => ({ status: "ok" as const, data: null }),
    settingGet: async () => null,
  });
});

const reply = (
  value: AccountV1,
  fault: AccountReplyV1["fault"] = null,
  retry: number | null = null,
): AccountReplyV1 => ({ account: value, fault, retry_after: retry });

const base = accountFixture();
const waiting = accountFixture({ pending_email: "ada@example.com", resend_in: 42 });
const ready = accountFixture({ pending_email: "ada@example.com", resend_in: null });

const shots = "../../../../../target/sign-in";
const settle = (ms = 700) => new Promise((done) => setTimeout(done, ms));

type Screen = Awaited<ReturnType<typeof render>>;

/** Never answers until `release` is called: the state in flight. */
function held() {
  let release: (value: AccountReplyV1) => void = () => {};
  const promise = new Promise<AccountReplyV1>((resolve) => (release = resolve));
  return { promise, release };
}

async function shoot(
  name: string,
  account: AccountV1,
  act?: (screen: Screen) => Promise<void>,
  size: [number, number] = [1000, 760],
) {
  native.get.mockResolvedValue(reply(account));
  await page.viewport(...size);
  const screen = await render(AccountPageStage);
  await settle();
  await act?.(screen);
  for (const theme of ["dark", "light"]) {
    document.documentElement.dataset.theme = theme;
    await settle(500);
    await page.screenshot({ path: `${shots}/${name}-${theme}.png` });
  }
  document.documentElement.dataset.theme = "dark";
  await screen.unmount();
}

test("Account sign-in: every state, dark and light", async () => {
  native.send.mockResolvedValue(reply(waiting));
  native.cancel.mockResolvedValue(reply(base));
  await shoot("1-email", base);
  await shoot("2-email-typed", base, async (screen) => {
    await screen.getByLabelText("Email address").fill("ada@example.com");
  });
  const sending = held();
  await shoot("3-sending", base, async (screen) => {
    native.send.mockReturnValueOnce(sending.promise);
    await screen.getByLabelText("Email address").fill("ada@example.com");
    await screen.getByRole("button", { name: "Continue" }).click();
    await settle(300);
  });
  sending.release(reply(base));
  await settle(100);
  await shoot("4-code", waiting);
  await shoot("5-code-typed", waiting, async (screen) => {
    await screen.getByLabelText("Code").fill("482");
  });
  const verifying = held();
  await shoot("6-verifying", waiting, async (screen) => {
    native.verify.mockReturnValueOnce(verifying.promise);
    await screen.getByLabelText("Code").click();
    await userEvent.keyboard("482913");
    await settle(300);
  });
  verifying.release(reply(waiting));
  await settle(100);
  await shoot("7-wrong-code", waiting, async (screen) => {
    native.verify.mockResolvedValueOnce(reply(waiting, "invalid_code"));
    await screen.getByLabelText("Code").click();
    await userEvent.keyboard("482913");
    await settle(400);
  });
  await shoot("8-code-resend-ready", ready);
  await shoot("9-code-resent", ready, async (screen) => {
    native.send.mockResolvedValueOnce(reply(waiting));
    await screen.getByRole("button", { name: "Send a new code" }).click();
    await settle(300);
  });
  await shoot("10-offline", base, async (screen) => {
    native.send.mockResolvedValueOnce(reply(base, "offline"));
    await screen.getByLabelText("Email address").fill("ada@example.com");
    await screen.getByRole("button", { name: "Continue" }).click();
    await settle(300);
  });
  await shoot("11-rate-limited", waiting, async (screen) => {
    native.verify.mockResolvedValueOnce(reply(waiting, "rate_limited", 38));
    await screen.getByLabelText("Code").click();
    await userEvent.keyboard("111111");
    await settle(300);
  });
  await shoot("12-invite-only", accountFixture({ signup_mode: "allowlist" }));
  await shoot("13-closed", accountFixture({ signup_mode: "closed", signup_open: false }));
  await shoot("14-update-required", accountFixture({ update_required: true }));
  // The two steps trade places: frozen a third of the way through, so the
  // overlap can be judged.
  await shoot("14b-step-midway", base, async (screen) => {
    native.send.mockResolvedValueOnce(reply(waiting));
    await screen.getByLabelText("Email address").fill("ada@example.com");
    await screen.getByRole("button", { name: "Continue" }).click();
    await settle(40);
    for (const animation of document.getAnimations()) {
      animation.pause();
      animation.currentTime = 130;
    }
  });
  await shoot("15-narrow", waiting, undefined, [560, 720]);
  await shoot("16-short", base, undefined, [1000, 480]);
  await shoot("17-wide", base, undefined, [1500, 960]);
  // What signing in leads to is as it was: the sky is only the way in.
  await shoot(
    "18-signed-in",
    accountFixture({
      signed_in: true,
      email: "ada@example.com",
      name: "Ada Lovelace",
      plan: "free",
      usage: usageFixture(),
      subscription: {
        status: "inactive",
        plan: null,
        interval: null,
        paid_until: null,
        cancel_at_period_end: false,
      },
    }),
    undefined,
    [1000, 900],
  );
}, 240_000);
