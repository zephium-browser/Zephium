import "$styles/global.css";
import { beforeEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page, userEvent } from "vitest/browser";
import type { AccountReplyV1, AccountV1 } from "$shared/ipc/bindings";
import { accountFixture as account } from "$shared/testing/account";
import AccountPage from "../components/sections/AccountPage.svelte";

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
    profilesGet: async () => ({ status: "ok" as const, data: null }),
    settingGet: async () => null,
  });
});

const reply = (value: AccountV1, fault: AccountReplyV1["fault"] = null): AccountReplyV1 => ({
  account: value,
  fault,
  retry_after: null,
});

const waiting = account({ pending_email: "ada@example.com", resend_in: 60 });

beforeEach(() => {
  native.get.mockReset().mockResolvedValue(reply(account()));
  native.send.mockReset().mockResolvedValue(reply(waiting));
  native.verify.mockReset();
  native.cancel.mockReset().mockResolvedValue(reply(account()));
});

/** Every transition and animation in flight has finished or been cancelled. */
const settled = () =>
  Promise.all(
    document.getAnimations().map((animation) => animation.finished.catch(() => undefined)),
  );

const sheet = () => document.querySelector<HTMLElement>(".sign-in")!;
const height = () => sheet().getBoundingClientRect().height;
const pane = (name: string) =>
  sheet().querySelector<HTMLElement>(
    `.panes .pane:has(${name === "email" ? "[type=email]" : ".code"})`,
  )!;

test("both steps stand in one sheet that keeps its size, with the other waiting inert", async () => {
  await page.viewport(1000, 800);
  const screen = await render(AccountPage);
  const email = screen.getByLabelText("Email address");
  await expect.element(email).toBeVisible();
  await settled();
  const size = height();
  expect(sheet().dataset.step).toBe("email");
  expect(pane("code").inert).toBe(true);

  await email.fill("ada@example.com");
  await screen.getByRole("button", { name: "Continue" }).click();
  await expect.element(screen.getByLabelText("Code")).toHaveFocus();
  expect(sheet().dataset.step).toBe("code");
  expect(pane("email").inert).toBe(true);
  expect(pane("code").inert).toBe(false);
  expect(height()).toBeCloseTo(size, 0);

  await screen.getByRole("button", { name: "Use a different email" }).click();
  await expect.element(email).toHaveFocus();
  await expect.element(email).toHaveValue("ada@example.com");
  expect(sheet().dataset.step).toBe("email");
  expect(height()).toBeCloseTo(size, 0);
});

test("a fault arrives in room the sheet keeps, and marks the field it is about", async () => {
  await page.viewport(1000, 800);
  native.get.mockResolvedValue(reply(waiting));
  native.verify.mockResolvedValue(reply(waiting, "invalid_code"));
  const screen = await render(AccountPage);
  const code = screen.getByLabelText("Code");
  await expect.element(code).toBeVisible();
  const size = height();
  expect(pane("code").querySelector("form")!.dataset.invalid).toBe("false");

  await code.click();
  await userEvent.keyboard("482913");
  await expect
    .element(screen.getByRole("alert"))
    .toHaveTextContent("That code didn’t work. Check it, or send a new one.");
  expect(pane("code").querySelector("form")!.dataset.invalid).toBe("true");
  expect(height()).toBeCloseTo(size, 0);
  await expect.element(code).toHaveFocus();
});

test("a fault that is not about the field leaves the field alone", async () => {
  await page.viewport(1000, 800);
  native.send.mockResolvedValue(reply(account(), "offline"));
  const screen = await render(AccountPage);
  const email = screen.getByLabelText("Email address");
  await expect.element(email).toBeVisible();
  await settled();
  const size = height();
  await email.fill("ada@example.com");
  await screen.getByRole("button", { name: "Continue" }).click();
  await expect
    .element(screen.getByRole("alert"))
    .toHaveTextContent("Couldn’t reach the account server.");
  expect(pane("email").querySelector("form")!.dataset.invalid).toBe("false");
  expect(height()).toBeCloseTo(size, 0);
});

test("what the page has to say stands in the sheet, not above it", async () => {
  await page.viewport(1000, 800);
  native.get.mockResolvedValue(reply(account({ update_required: true })));
  const screen = await render(AccountPage);
  await expect
    .element(screen.getByText("This server asks for a newer version of Zephium.", { exact: false }))
    .toBeVisible();
  expect(sheet().textContent).toContain("Update Zephium to keep using your account here.");
});

test("the steps trade by opacity and a slide, and by nothing when motion is reduced", async () => {
  await page.viewport(1000, 800);
  const screen = await render(AccountPage);
  await expect.element(screen.getByLabelText("Email address")).toBeVisible();
  const waits = pane("code");
  const properties = getComputedStyle(waits)
    .transitionProperty.split(",")
    .map((p) => p.trim());
  expect(properties.sort()).toEqual(["opacity", "translate", "visibility"]);
  expect(getComputedStyle(waits).translate).not.toBe("none");
  expect(getComputedStyle(waits).transitionDelay).toContain("0.4s");

  // The global switch takes the durations; this takes the wait before a
  // step is hidden.
  document.documentElement.dataset.reduceMotion = "true";
  try {
    const durations = getComputedStyle(waits).transitionDuration.split(",");
    expect(durations.every((duration) => Number.parseFloat(duration) < 0.001)).toBe(true);
    expect(
      getComputedStyle(waits)
        .transitionDelay.split(",")
        .every((d) => d.trim() === "0s"),
    ).toBe(true);
  } finally {
    delete document.documentElement.dataset.reduceMotion;
  }
});

test("the sky is the page's backdrop and its name stays for assistive technology", async () => {
  await page.viewport(1000, 800);
  const screen = await render(AccountPage);
  await expect.element(screen.getByLabelText("Email address")).toBeVisible();
  const gate = document.querySelector<HTMLElement>(".account-gate")!;
  await expect.poll(() => gate.dataset.ready, { timeout: 8000 }).toBe("true");
  expect(gate.dataset.sky).toBe("night");
  expect(gate.querySelector<HTMLElement>(".sky")!.style.backgroundImage).toContain("sky-night");
  await expect.element(screen.getByRole("img", { name: "Zephium" })).toBeVisible();
});
