import "$styles/global.css";
import { afterEach, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page } from "vitest/browser";
import type { AccountV1, ProfileEntryView, ProfilesView } from "$shared/ipc/bindings";
import { accountFixture, usageFixture } from "$shared/testing/account";
import { preferences } from "$domain/preferences";
import * as settings from "../lib/settings-state.svelte";
import type { SettingsSection } from "../lib/settings-model";
import SettingsLookStage from "./SettingsLookStage.svelte";

const native = vi.hoisted(() => ({
  account: null as AccountV1 | null,
  profiles: null as ProfilesView | null,
  settings: {} as Record<string, string>,
}));
// A look bench: commands a shot depends on answer for real, the rest nothing.
vi.mock("$shared/ipc/bindings", async () => {
  const known: Record<string, (...args: never[]) => Promise<unknown>> = {
    accountGet: async () => ({ account: native.account!, fault: null, retry_after: null }),
    accountRefresh: async () => ({ account: native.account!, fault: null, retry_after: null }),
    profilesGet: async () => ({ status: "ok", data: native.profiles }),
    settingGet: async (key: never) => native.settings[key] ?? null,
    keymapEntries: async () => [],
    runCommand: async () => ({ accepted: true, operation_id: null }),
  };
  return {
    commands: new Proxy(known, {
      get: (target, key) =>
        typeof key === "string" ? (target[key] ?? (async () => null)) : undefined,
    }),
  };
});

/** Photos are served natively; here a painted one stands in for it. */
const photo = vi.hoisted(() => ({ url: "" }));
vi.mock("$domain/profiles/avatar", async (original) => {
  const actual = await original<typeof import("$domain/profiles")>();
  const swap = <T extends { style: string; src: string | null }>(view: T): T =>
    view.style === "photo" ? { ...view, src: photo.url } : view;
  return {
    ...actual,
    drawn: (profile: Parameters<typeof actual.drawn>[0]) => swap(actual.drawn(profile)),
    identityAvatar: (...args: Parameters<typeof actual.identityAvatar>) =>
      swap(actual.identityAvatar(...args)),
  };
});

function paintPhoto(): string {
  const canvas = document.createElement("canvas");
  canvas.width = canvas.height = 256;
  const context = canvas.getContext("2d")!;
  const sky = context.createLinearGradient(0, 0, 0, 256);
  sky.addColorStop(0, "#f6b48f");
  sky.addColorStop(0.55, "#e9877a");
  sky.addColorStop(1, "#5b4a6b");
  context.fillStyle = sky;
  context.fillRect(0, 0, 256, 256);
  context.fillStyle = "#ffe2b8";
  context.beginPath();
  context.arc(150, 120, 34, 0, Math.PI * 2);
  context.fill();
  context.fillStyle = "#3a3350";
  context.beginPath();
  context.moveTo(0, 190);
  context.quadraticCurveTo(80, 140, 150, 180);
  context.quadraticCurveTo(210, 210, 256, 170);
  context.lineTo(256, 256);
  context.lineTo(0, 256);
  context.fill();
  context.fillStyle = "#231f33";
  context.beginPath();
  context.moveTo(0, 230);
  context.quadraticCurveTo(120, 190, 256, 225);
  context.lineTo(256, 256);
  context.lineTo(0, 256);
  context.fill();
  return canvas.toDataURL("image/png");
}

const ID = (n: number) => `01J${String(n).padStart(23, "0")}`;
const profile = (
  n: number,
  name: string,
  kind: ProfileEntryView["kind"],
  avatar: Partial<ProfileEntryView["avatar"]>,
): ProfileEntryView => ({
  id: ID(n),
  name,
  kind,
  avatar: { style: "initials", color: 1, symbol: 0, revision: 1, ...avatar },
});
const roster: ProfilesView = {
  current: ID(1),
  private: false,
  profiles: [
    profile(1, "Ada Lovelace", "default", { style: "photo", revision: 3 }),
    profile(2, "Work", "named", { style: "symbol", color: 1, symbol: 0 }),
    profile(3, "Side Project", "named", { style: "initials", color: 8 }),
  ],
};

const signedIn = (): AccountV1 => ({
  ...accountFixture(),
  signed_in: true,
  email: "ada@example.com",
  name: "Ada Lovelace",
  plan: "pro",
  usage: usageFixture({ used_percent: 38, allowance_used: "760000" }),
  subscription: {
    status: "active",
    plan: null,
    interval: "monthly",
    paid_until: "2026-11-02T00:00:00Z",
    cancel_at_period_end: false,
  },
});

const shots = "../../../../../target/look/profiles";
const settle = (ms = 450) => new Promise((done) => setTimeout(done, ms));
const looks: [string, "dark" | "light", string | null][] = [
  ["zephium", "dark", null],
  ["zephium", "light", null],
  ["nord", "dark", "nord"],
];

async function look(name: string, act: () => Promise<void>) {
  for (const [id, mode, theme] of looks) {
    native.settings = theme ? { "ui.theme": theme, appearance: mode } : { appearance: mode };
    document.documentElement.dataset.theme = mode;
    await preferences.init();
    await act();
    await settle(300);
    await page.screenshot({ path: `${shots}/${name}-${id}-${mode}.png` });
    preferences.dispose();
  }
}

afterEach(() => {
  document.documentElement.dataset.theme = "dark";
});

async function settingsAt(section: SettingsSection) {
  const screen = await render(SettingsLookStage);
  settings.select(section);
  await settle();
  return screen;
}

test("Profiles: three profiles, then the create and edit sheets", async () => {
  photo.url = paintPhoto();
  native.account = accountFixture();
  native.profiles = roster;
  await page.viewport(1280, 860);
  const screen = await settingsAt("profiles");
  await look("profiles", async () => {});
  await screen.getByRole("button", { name: "New Profile…" }).click();
  await settle();
  await screen.getByLabelText("Name").fill("School");
  await screen.getByRole("button", { name: "Symbol 10" }).click();
  await screen.getByRole("button", { name: "Color 4" }).click();
  await look("create-sheet", async () => {});
  await screen.getByRole("button", { name: "Cancel" }).click();
  await settle();
  await screen.getByRole("button", { name: "Edit Ada Lovelace" }).click();
  await look("edit-photo-sheet", async () => {});
  await screen.getByRole("button", { name: "Cancel" }).click();
  await settle();
  await screen.getByRole("button", { name: "Edit Work" }).click();
  await settle();
  await screen.getByRole("button", { name: "Delete Profile…" }).click();
  await look("delete-confirm", async () => {});
  await screen.unmount();
});

test("Account: signed out, the code step, and signed in with a plan", async () => {
  photo.url = paintPhoto();
  native.profiles = roster;
  await page.viewport(1280, 860);
  native.account = accountFixture();
  let screen = await settingsAt("account");
  await look("account-signed-out", async () => {});
  await screen.unmount();

  native.account = { ...accountFixture(), pending_email: "ada@example.com", resend_in: 42 };
  screen = await settingsAt("account");
  await look("account-code", async () => {});
  await screen.unmount();

  native.account = signedIn();
  screen = await settingsAt("account");
  await look("account-signed-in", async () => {});
  const scroller = document.querySelector<HTMLElement>(".settings-content")!;
  await look("account-signed-in-lower", async () => {
    scroller.scrollTop = scroller.scrollHeight;
  });
  await screen.unmount();

  native.profiles = {
    ...roster,
    profiles: [
      profile(1, "Personal", "default", { revision: 0, color: 4 }),
      ...roster.profiles.slice(1),
    ],
  };
  screen = await settingsAt("account");
  await look("account-unchosen-avatar", async () => {});
  await screen.unmount();
});
