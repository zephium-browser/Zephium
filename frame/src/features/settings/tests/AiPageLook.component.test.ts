import "$styles/global.css";
import { afterEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page } from "vitest/browser";
import AiPageStage from "./AiPageStage.svelte";
import { preferences } from "$domain/preferences";
import { emitNativeEvent } from "$shared/testing/native-events";
import * as settings from "../lib/settings-state.svelte";
import SettingsLookStage from "./SettingsLookStage.svelte";

vi.mock("$domain/tabs", () => ({
  tabs: { profile: () => ({ id: "00000000000000000000000001", kind: "regular" }) },
}));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  const { models } = await import("$shared/testing/work-models");
  const { accountFixture } = await import("$shared/testing/account");
  return mockBindings({
    accountGet: async () => ({ account: accountFixture(), fault: null, retry_after: null }),
    voiceStatus: async () => ({
      version: 1,
      engines: ["on_device", "open_ai"],
      chosen: null,
      effective: "on_device",
      language: null,
      languages: ["en-US"],
      on_device: "en_US",
      microphone: "granted",
    }),
    workDecisionPreference: async () => ({
      version: 1,
      profile: "00000000000000000000000001",
      choice: "recommended",
      effective: "standard",
      typesafe_key_present: false,
      error: null,
    }),
    workModels: vi.fn(async () =>
      models({
        keys: { anthropic: "valid", open_ai: "set", deep_seek: "invalid" },
        base: "http://localhost:11434/v1",
        lead: "anthropic/claude-opus-5-5",
      }),
    ),
  });
});

const shots = "../../../../../target/work-models";
const settle = () => new Promise((done) => setTimeout(done, 500));

afterEach(() => preferences.dispose());

test("visibility comparison for hosted-model invitations", async () => {
  await preferences.init();
  document.documentElement.dataset.theme = "dark";
  await page.viewport(1100, 880);
  settings.select("ai");
  const screen = await render(SettingsLookStage);
  await expect.element(screen.getByRole("button", { name: "Sign in…", exact: true })).toBeVisible();
  await settle();
  await page.screenshot({ path: "../../../../../target/look/visibility/models-before.png" });
  emitNativeEvent("uiCommand", "preference.ui.sign-in-prompts=false");
  await expect
    .element(screen.getByRole("button", { name: "Sign in…", exact: true }))
    .not.toBeInTheDocument();
  await settle();
  await page.screenshot({ path: "../../../../../target/look/visibility/models-after.png" });
});

test("Settings → AI with BYOK keys and model roles, in both themes", async () => {
  await page.viewport(1100, 1300);
  const screen = await render(AiPageStage);
  for (const theme of ["dark", "light"]) {
    document.documentElement.dataset.theme = theme;
    await settle();
    await page.screenshot({ path: `${shots}/settings-${theme}.png` });
  }
  await screen.getByRole("button", { name: "Add key" }).first().click();
  for (const theme of ["dark", "light"]) {
    document.documentElement.dataset.theme = theme;
    await settle();
    await page.screenshot({ path: `${shots}/settings-add-key-${theme}.png` });
  }
  document.documentElement.dataset.theme = "dark";
});
