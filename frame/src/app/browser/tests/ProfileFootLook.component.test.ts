import { test, vi } from "vitest";
import { page } from "vitest/browser";
import { render } from "vitest-browser-svelte";
import type { ProfileEntryView, ProfilesView } from "$shared/ipc/bindings";
import ProfileFootLook from "./ProfileFootLook.svelte";

const native = vi.hoisted(() => ({ profiles: null as ProfilesView | null }));
vi.mock("$shared/ipc/bindings", async () => {
  const known: Record<string, (...args: never[]) => Promise<unknown>> = {
    profilesGet: async () => ({ status: "ok", data: native.profiles }),
    runCommand: async () => ({ accepted: true, operation_id: null }),
    toolsMenuPopup: async () => true,
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

const photo = vi.hoisted(() => ({ url: "" }));
vi.mock("$domain/profiles/avatar", async (original) => {
  const actual = await original<typeof import("$domain/profiles")>();
  return {
    ...actual,
    drawn: (profile: Parameters<typeof actual.drawn>[0]) => {
      const view = actual.drawn(profile);
      return view.style === "photo" ? { ...view, src: photo.url } : view;
    },
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
  return canvas.toDataURL("image/png");
}

const ID = (n: number) => `01J${String(n).padStart(23, "0")}`;
const entry = (
  n: number,
  name: string,
  avatar: Partial<ProfileEntryView["avatar"]>,
): ProfileEntryView => ({
  id: ID(n),
  name,
  kind: n === 1 ? "default" : "named",
  avatar: { style: "initials", color: 1, symbol: 0, revision: 1, ...avatar },
});
const profiles = [
  entry(1, "Ada Lovelace", { style: "photo", revision: 3 }),
  entry(2, "Work", { style: "symbol", color: 1, symbol: 0 }),
  entry(3, "Side Project", { style: "initials", color: 8 }),
];

const settle = (ms = 400) => new Promise((done) => setTimeout(done, ms));
const shots = "../../../../../target/look/profiles";

async function shoot(target: HTMLElement, name: string) {
  for (const theme of ["dark", "light"]) {
    document.documentElement.dataset.theme = theme;
    await settle(200);
    await page.elementLocator(target).screenshot({ path: `${shots}/${name}-${theme}.png` });
  }
  document.documentElement.dataset.theme = "dark";
}

test("the sidebar foot wears the profile's photo, and its stack lists the other profiles", async () => {
  photo.url = paintPhoto();
  await page.viewport(520, 720);
  native.profiles = { current: ID(1), private: false, profiles };
  const screen = await render(ProfileFootLook);
  await settle();
  await shoot(screen.container, "foot-photo");
  await screen
    .getByRole("button", { name: /^Tools/u })
    .first()
    .click();
  await settle();
  await shoot(screen.container, "foot-photo-open");
  await screen.unmount();

  native.profiles = { current: ID(2), private: false, profiles };
  const symbol = await render(ProfileFootLook);
  await settle();
  await shoot(symbol.container, "foot-symbol");
  await symbol.unmount();

  native.profiles = {
    current: ID(1),
    private: false,
    profiles: [entry(1, "Personal", { revision: 0 }), ...profiles.slice(1)],
  };
  const unchosen = await render(ProfileFootLook);
  await settle();
  await shoot(unchosen.container, "foot-unchosen");
  await unchosen.unmount();
});
