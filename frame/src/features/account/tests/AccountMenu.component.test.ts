import { afterEach, expect, test, vi } from "vitest";
import { page } from "vitest/browser";
import { render } from "vitest-browser-svelte";
import type { AccountV1, ProfileEntryView, ProfilesView } from "$shared/ipc/bindings";
import { accountFixture, usageFixture } from "$shared/testing/account";
import { preferences } from "$domain/preferences";
import { emitNativeEvent } from "$shared/testing/native-events";
import AccountMenuStage from "./AccountMenuStage.svelte";

const native = vi.hoisted(() => ({
  account: null as unknown as AccountV1,
  profiles: null as unknown as ProfilesView,
  command: vi.fn(async (_id: string) => ({ operation_id: "op", accepted: true })),
  switch: vi.fn(async (_id: string) => ({ status: "ok", data: null })),
}));

vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    accountGet: vi.fn(async () => ({ account: native.account, fault: null, retry_after: null })),
    profilesGet: vi.fn(async () => ({ status: "ok" as const, data: native.profiles })),
    profilesSwitch: native.switch as never,
    runCommand: native.command as never,
  });
});

vi.mock("$domain/keymap", () => ({
  keymap: { all: () => [{ id: "browser.settings", accelerator: "CmdOrCtrl+," }] },
}));

const ID = (n: number) => `01J${String(n).padStart(23, "0")}`;
const entry = (n: number, name: string, color: number): ProfileEntryView => ({
  id: ID(n),
  name,
  kind: n === 1 ? "default" : "named",
  avatar: { style: "initials", color, symbol: 0, revision: 1 },
});
const solo = [entry(1, "Ada Lovelace", 1)];
const three = [solo[0]!, entry(2, "Work", 4), entry(3, "Side Project", 8)];

const shots = "../../../../../target/look/account-menu";
const settle = (ms = 500) => new Promise((done) => setTimeout(done, ms));

async function shoot(name: string) {
  for (const theme of ["dark", "light"]) {
    document.documentElement.dataset.theme = theme;
    await settle();
    await page.screenshot({ path: `${shots}/${name}-${theme}.png` });
  }
  document.documentElement.dataset.theme = "dark";
}

async function open(account: AccountV1, profiles: ProfileEntryView[], name = "Account") {
  native.account = account;
  native.profiles = { current: ID(1), private: false, profiles };
  const screen = await render(AccountMenuStage);
  await settle(200);
  await screen.getByRole("button", { name, exact: true }).click();
  await expect.element(page.getByRole("menuitem", { name: /^Settings/u })).toBeVisible();
  await expect.poll(() => document.querySelector<HTMLElement>(".work-menu")?.inert).toBe(false);
  return screen;
}

const signedIn = accountFixture({
  signed_in: true,
  email: "ada@example.com",
  plan: "pro",
  usage: usageFixture(),
});

test("the menu leads with who you are, then account, models and settings", async () => {
  await page.viewport(520, 420);
  native.command.mockClear();
  const screen = await open(signedIn, three);

  const items = page.getByRole("menuitem").all();
  expect(
    items.map(
      (item) => item.element().getAttribute("aria-label") ?? item.element().textContent?.trim(),
    ),
  ).toEqual([
    "Ada Lovelace — Profile Settings",
    "Switch to Work",
    "Switch to Side Project",
    "Account — ada@example.com",
    "Models",
    expect.stringContaining("Settings"),
  ]);
  await expect
    .element(page.getByRole("menuitem", { name: /^Account/u }))
    .toHaveTextContent(/ada@example\.com.*Pro.*42% used/u);
  await expect
    .element(page.getByRole("meter", { name: "Included usage used" }))
    .toHaveAttribute("aria-valuenow", "42");
  expect(page.getByText("History").query()).toBeNull();
  expect(page.getByText("Downloads").query()).toBeNull();

  await page.getByRole("menuitem", { name: /Profile Settings/u }).click();
  expect(native.command).toHaveBeenLastCalledWith("settings.profiles");
  await screen.getByRole("button", { name: "Account", exact: true }).click();
  await page.getByRole("menuitem", { name: "Models" }).click();
  expect(native.command).toHaveBeenLastCalledWith("settings.ai");
  await screen.getByRole("button", { name: "Account", exact: true }).click();
  await page.getByRole("menuitem", { name: /^Account/u }).click();
  expect(native.command).toHaveBeenLastCalledWith("settings.account");
  await screen.getByRole("button", { name: "Account", exact: true }).click();
  await page.getByRole("menuitem", { name: /^Settings/u }).click();
  expect(native.command).toHaveBeenLastCalledWith("browser.settings");
});

test("another profile is one click away", async () => {
  await page.viewport(520, 420);
  native.switch.mockClear();
  await open(signedIn, three);
  await page.getByRole("menuitem", { name: "Switch to Side Project" }).click();
  expect(native.switch).toHaveBeenCalledWith(ID(3));
});

test("a lone profile has no one to switch to, and signed out offers to sign in", async () => {
  await page.viewport(520, 420);
  await open(accountFixture(), solo);
  expect(page.getByRole("menuitem", { name: /Switch to/u }).query()).toBeNull();
  await expect
    .element(page.getByRole("menuitem", { name: "Sign in to Zephium", exact: true }))
    .toBeVisible();
  expect(page.getByRole("menuitem", { name: /^Account/u }).query()).toBeNull();
});

test("the menu is part of the keyboard", async () => {
  await page.viewport(520, 420);
  native.command.mockClear();
  const screen = await open(signedIn, solo);
  await expect.element(page.getByRole("menuitem", { name: /Profile Settings/u })).toHaveFocus();
  await expect
    .element(page.getByRole("button", { name: "Account", exact: true }))
    .toHaveAttribute("aria-expanded", "true");
  await page.getByRole("menuitem", { name: /Profile Settings/u }).click();
  await screen.getByRole("button", { name: "Account", exact: true }).click();
  const { userEvent } = await import("vitest/browser");
  await userEvent.keyboard("{Escape}");
  await expect
    .element(page.getByRole("button", { name: "Account", exact: true }))
    .toHaveAttribute("aria-expanded", "false");
});

test(
  "what the menu looks like, signed in and out, with one profile and three",
  { timeout: 90_000 },
  async () => {
    await page.viewport(520, 420);
    for (const [name, account, profiles] of [
      ["in-three", signedIn, three],
      ["in-one", signedIn, solo],
      ["out-three", accountFixture(), three],
      ["out-one", accountFixture(), solo],
      [
        "usage-high",
        accountFixture({
          signed_in: true,
          email: "ada@example.com",
          plan: "pro",
          usage: usageFixture({ used_percent: 91 }),
        }),
        three,
      ],
      [
        "usage-full",
        accountFixture({
          signed_in: true,
          email: "ada@example.com",
          plan: "pro",
          usage: usageFixture({ used_percent: 100 }),
        }),
        three,
      ],
      [
        "in-long",
        accountFixture({
          signed_in: true,
          email: "ada.lovelace@analytical-engine.example",
          plan: "pro",
          usage: usageFixture(),
        }),
        three,
      ],
    ] as const) {
      const screen = await open(account, [...profiles]);
      await shoot(name);
      await screen.unmount();
    }
  },
);

afterEach(() => preferences.dispose());

test("hidden sign-in prompts preserve profile, model and settings actions", async () => {
  await preferences.init();
  await page.viewport(520, 420);
  document.documentElement.dataset.theme = "dark";
  await open(accountFixture(), solo);
  await expect
    .element(page.getByRole("menuitem", { name: "Sign in to Zephium", exact: true }))
    .toBeVisible();
  await settle();
  await page.screenshot({ path: "../../../../../target/look/visibility/profile-menu-before.png" });
  emitNativeEvent("uiCommand", "preference.ui.sign-in-prompts=false");
  await expect
    .element(page.getByRole("menuitem", { name: "Sign in to Zephium", exact: true }))
    .not.toBeInTheDocument();
  await settle();
  await page.screenshot({ path: "../../../../../target/look/visibility/profile-menu-after.png" });
  await expect.element(page.getByRole("menuitem", { name: /Profile Settings/u })).toBeVisible();
  await expect.element(page.getByRole("menuitem", { name: "Models", exact: true })).toBeVisible();
});
