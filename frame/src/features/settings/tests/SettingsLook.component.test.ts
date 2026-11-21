import "$styles/global.css";
import { test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page } from "vitest/browser";
import type { AccountV1, LoginItemStatus } from "$shared/ipc/bindings";
import { accountFixture } from "$shared/testing/account";
import * as settings from "../lib/settings-state.svelte";
import type { SettingsSection } from "../lib/settings-model";
import SettingsLookStage from "./SettingsLookStage.svelte";

const native = vi.hoisted(() => ({
  account: null as AccountV1 | null,
  login: { supported: true, enabled: false, needs_approval: false } as LoginItemStatus,
}));
// A look bench, not a contract test: commands the pages read in passing
// answer nothing, and the ones the shot depends on answer for real.
vi.mock("$shared/ipc/bindings", async () => {
  const known: Record<string, (...args: never[]) => Promise<unknown>> = {
    accountGet: async () => ({ account: native.account!, fault: null, retry_after: null }),
    loginItemStatus: async () => native.login,
    defaultBrowserStatus: async () => ({ is_default: false, can_request: true }),
    aboutInfo: async () => ({ version: "1.0.0-beta.2", os: "macOS 26.1", arch: "arm64" }),
    updateStatus: async () => ({ state: "upToDate" }),
    settingGet: async () => null,
    keymapEntries: async () => [],
  };
  return {
    commands: new Proxy(known, {
      get: (target, key) =>
        typeof key === "string" ? (target[key] ?? (async () => null)) : undefined,
    }),
  };
});

const shots = "../../../../../target/look/settings";
const settle = (ms = 450) => new Promise((done) => setTimeout(done, ms));

async function shoot(name: string, section: SettingsSection) {
  settings.select(section);
  await settle();
  for (const theme of ["dark", "light"]) {
    document.documentElement.dataset.theme = theme;
    await settle(250);
    await page.screenshot({ path: `${shots}/${name}-${theme}.png` });
  }
  document.documentElement.dataset.theme = "dark";
}

test("Settings pages in their final shape, both themes", async () => {
  await page.viewport(1280, 860);
  native.account = accountFixture();
  let screen = await render(SettingsLookStage);
  await settle();
  await shoot("general", "general");
  await shoot("tabs", "tabs");
  await shoot("newtab-empty", "newtab");
  await screen.getByRole("textbox", { name: "New tab URL" }).fill("https://example.com/start");
  await shoot("newtab-filled", "newtab");
  await page.viewport(740, 720);
  await shoot("newtab-narrow", "newtab");
  await page.viewport(1280, 860);
  await shoot("focus", "focus");
  await shoot("about", "about");
  await shoot("appearance", "appearance");
  native.login = { supported: true, enabled: true, needs_approval: true };
  await shoot("general-approval", "general");
  await screen.unmount();

  native.account = accountFixture({
    signed_in: true,
    email: "ada@example.com",
    name: "Ada Lovelace",
    plan: "pro",
  });
  screen = await render(SettingsLookStage);
  await settle();
  await shoot("account-signed-in", "account");
  await screen.unmount();
});
