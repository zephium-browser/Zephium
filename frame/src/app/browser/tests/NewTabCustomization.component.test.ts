import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page } from "vitest/browser";
import { createRawSnippet } from "svelte";
import "$styles/global.css";
import { preferences } from "$domain/preferences";
import { retryImage, newTabImage } from "$session/newtab-background.svelte";
import { emitNativeEvent } from "$shared/testing/native-events";
import { resourceTestServer } from "$shared/testing/resources/server";
import type { MediaImportV1_Deserialize } from "$shared/ipc/bindings";
import { loadNewTab } from "$features/newtab";
import { loadSettings, handleNativeSection } from "$features/settings";

async function settingsScreen() {
  const { default: Settings } = await loadSettings();
  handleNativeSection("settings.section.newtab");
  return render(Settings);
}

const native = vi.hoisted(() => ({
  settings: {} as Record<string, string>,
  profile: "00000000000000000000000901",
  import: vi.fn(),
  media: vi.fn(),
  decode: vi.fn(),
  blocked: vi.fn(),
  time: vi.fn(),
}));
vi.mock("$domain/tabs", () => ({
  tabs: { profile: () => ({ id: native.profile, kind: "default", name: "Alex" }) },
}));
vi.mock("$shared/sky", async (original) => {
  const real = await original<typeof import("$shared/sky")>();
  return {
    ...real,
    decode: (scene: import("$shared/sky").Scene) => {
      native.decode(scene);
      return real.decode(scene);
    },
  };
});
vi.mock("$domain/resources", async (original) => ({
  ...(await original<Record<string, unknown>>()),
  mediaUrl: native.media,
}));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    settingGet: async (key) => native.settings[key] ?? null,
    settingSet: async (key, value) => {
      native.settings[key] = value;
      emitNativeEvent("uiCommand", `preference.${key}=${value}`);
      return { accepted: true, operation_id: null };
    },
    mediaImport: native.import,
    blockerStats: native.blocked,
    timeCall: native.time,
    resourceCall: (...args) => resourceTestServer(native.profile).call(...args),
  });
});
const digest = "a".repeat(64);
function imported(kind: "image" | "pdf" = "image"): MediaImportV1_Deserialize {
  return {
    kind: "imported",
    record: {
      id: "00000000000000000000000902",
      revision: "1",
      created_at: "1",
      updated_at: "1",
      trashed: false,
      draft: {
        related: [],
        title: "My image",
        pinned: false,
        content: {
          kind: "media",
          asset: {
            version: 1,
            kind,
            mime: kind === "image" ? "image/png" : "application/pdf",
            bytes: 100,
            digest,
            name: "image.png",
            origin: { kind: "imported" },
            width: 1600,
            height: 900,
          },
        },
      },
    },
  };
}
beforeEach(async () => {
  native.settings = {};
  native.profile = "00000000000000000000000901";
  native.import.mockReset().mockResolvedValue({ status: "ok", data: imported() });
  native.media.mockReset().mockReturnValue("/src/shared/sky/assets/sky-day.avif");
  native.decode.mockClear();
  native.blocked
    .mockReset()
    .mockResolvedValue({ status: "ok", data: { today: 0, last7Days: 0, days: [] } });
  native.time.mockReset().mockResolvedValue({ kind: "focus_days", days: [] });
  retryImage();
  await page.viewport(1280, 1000);
});
afterEach(() => {
  preferences.dispose();
  retryImage();
  delete document.documentElement.dataset.theme;
});
async function tab() {
  const { default: NewTab } = await loadNewTab();
  const screen = await render(NewTab, {
    search: createRawSnippet(() => ({ render: () => '<input aria-label="Search" />' })),
    oncustomize: () => {},
  });
  screen.container.style.cssText = "inline-size:1200px;block-size:800px";
  return screen;
}
async function chooseImage(screen: Awaited<ReturnType<typeof settingsScreen>>) {
  await screen.getByRole("button", { name: "Background", exact: true }).click();
  await page.getByRole("option", { name: "Your own image", exact: true }).click();
}

test("defaults do no image work, and every figure and wordmark updates live and resets", async () => {
  await preferences.init();
  const ntp = await tab();
  const settings = await settingsScreen();
  await expect.poll(() => ntp.container.querySelectorAll(".figure").length).toBe(3);
  expect(ntp.container.querySelector(".mark")).not.toBeNull();
  expect(native.media).not.toHaveBeenCalled();
  expect(ntp.container.querySelector("img")).toBeNull();
  const settingsSwitch = (name: string) => settings.getByRole("switch", { name, exact: true });
  await settingsSwitch("Wordmark").click();
  await expect.poll(() => ntp.container.querySelector(".mark")).toBeNull();
  for (const [index, label] of [
    "Ads & trackers blocked",
    "Focused",
    "Show what's due on Tasks",
  ].entries()) {
    await settingsSwitch(label).click();
    await expect.poll(() => ntp.container.querySelectorAll(".figure").length).toBe(2 - index);
  }
  await expect.poll(() => ntp.container.querySelector(".figures")).toBeNull();
  await preferences.set("ui.newtab-greeting", "true");
  await preferences.set("ui.newtab-name", "true");
  await preferences.set("ui.newtab-clock-format", "24h");
  await preferences.set("ui.newtab-clock", "false");
  await preferences.set("search.engine", "google");
  await settings.getByRole("button", { name: "Reset to default", exact: true }).click();
  await expect.poll(() => ntp.container.querySelectorAll(".figure").length).toBe(3);
  expect(ntp.container.querySelector(".mark")).not.toBeNull();
  expect(preferences.value("ui.newtab-greeting")).toBe("false");
  expect(preferences.value("ui.newtab-name")).toBe("false");
  expect(preferences.value("ui.newtab-clock")).toBe("true");
  expect(preferences.value("ui.newtab-clock-format")).toBe("system");
  expect(preferences.value("search.engine")).toBe("google");
});

test("a custom background uses one sized local image, no sky decode or frost, and dims live", async () => {
  native.settings = { "ui.newtab-background": "image", "ui.newtab-image": digest };
  await preferences.init();
  const ntp = await tab();
  const settings = await settingsScreen();
  await expect.poll(() => ntp.container.querySelector("img")?.naturalWidth ?? 0).toBeGreaterThan(0);
  expect(native.decode).not.toHaveBeenCalled();
  expect(native.media).toHaveBeenCalledTimes(1);
  expect(native.media.mock.calls[0]).toEqual([native.profile, digest, 1600]);
  expect(ntp.container.querySelector(".sky")).toBeNull();
  expect(
    ntp.container.querySelector<HTMLElement>(".newtab")!.style.getPropertyValue("--sky-frost"),
  ).toBe("");
  const image = ntp.container.querySelector("img")!;
  expect(getComputedStyle(image).objectFit).toBe("cover");
  expect(image.decoding).toBe("async");
  await settings.getByRole("switch", { name: "Dim", exact: true }).click();
  await expect.poll(() => ntp.container.querySelector(".dim")).not.toBeNull();
  expect(ntp.container.querySelector("img")).toBe(image);
  expect(native.media).toHaveBeenCalledTimes(1);
  await settings.getByRole("button", { name: "Reset to default", exact: true }).click();
  await expect.poll(() => ntp.container.querySelector(".sky")).not.toBeNull();
  expect(preferences.value("ui.newtab-image")).toBe("");
  expect(preferences.value("ui.newtab-dim")).toBe("false");
});

test("native image import commits only the admitted digest, and cancellation keeps the sky", async () => {
  await preferences.init();
  const settings = await settingsScreen();
  native.import.mockResolvedValueOnce({ status: "ok", data: { kind: "cancelled" } });
  await chooseImage(settings);
  await expect.poll(() => native.import.mock.calls.length).toBe(1);
  expect(preferences.value("ui.newtab-background")).toBe("sky");
  expect(preferences.value("ui.newtab-image")).toBe("");
  await chooseImage(settings);
  await expect.poll(() => preferences.value("ui.newtab-background")).toBe("image");
  expect(native.import).toHaveBeenLastCalledWith(native.profile);
  expect(preferences.value("ui.newtab-image")).toBe(digest);
});

test("non-images and failed imports keep the choice, and switching profile during the picker cannot apply it", async () => {
  await preferences.init();
  const settings = await settingsScreen();
  native.import.mockResolvedValueOnce({ status: "ok", data: imported("pdf") });
  await chooseImage(settings);
  await expect.element(settings.getByRole("alert")).toBeVisible();
  expect(preferences.value("ui.newtab-image")).toBe("");
  native.import.mockRejectedValueOnce(new Error("unavailable"));
  await chooseImage(settings);
  await expect.element(settings.getByRole("alert")).toBeVisible();
  expect(preferences.value("ui.newtab-background")).toBe("sky");
  native.import.mockImplementationOnce(async () => {
    native.profile = "00000000000000000000000903";
    return { status: "ok", data: imported() };
  });
  await chooseImage(settings);
  await expect.poll(() => native.import.mock.calls.length).toBe(3);
  expect(preferences.value("ui.newtab-image")).toBe("");
});

test("a missing or broken image silently restores the sky, including its material eligibility", async () => {
  native.settings = { "ui.newtab-background": "image", "ui.newtab-image": digest };
  native.media.mockReturnValue("/missing-newtab-image.png");
  await preferences.init();
  const ntp = await tab();
  await expect.poll(() => ntp.container.querySelector(".sky")).not.toBeNull();
  expect(newTabImage()).toBeNull();
  expect(ntp.container.querySelector("[role=alert]")).toBeNull();
  expect(ntp.container.querySelector("img")).toBeNull();
  expect(native.decode).toHaveBeenCalled();
});

test("settings follow the existing dark and light surfaces", async () => {
  native.settings = { "ui.newtab-background": "image", "ui.newtab-image": digest };
  await preferences.init();
  const settings = await settingsScreen();
  settings.container.style.cssText =
    "inline-size:760px;padding:24px;background:var(--color-canvas)";
  for (const theme of ["dark", "light"]) {
    document.documentElement.dataset.theme = theme;
    await page
      .elementLocator(settings.container)
      .screenshot({ path: `../../../../../target/newtab/customization-${theme}.png` });
  }
});
