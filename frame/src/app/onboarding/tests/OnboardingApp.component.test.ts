import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { page, userEvent } from "vitest/browser";
import { render } from "vitest-browser-svelte";
import "$styles/global.css";
import { emitNativeEvent } from "$shared/testing/native-events";
import { acceleratorKeys } from "$shared/lib/accelerator";
import { IS_MAC } from "$shared/platform";
import type { AccountReplyV1, Material, WorkModelsV1 } from "$shared/ipc/bindings";
import { browserImport, type ImportAdapter, type ImportJob } from "$domain/browser-import";
import { fakeTabs } from "./onboarding-tabs.svelte";
import OnboardingApp from "../OnboardingApp.svelte";

const native = vi.hoisted(() => ({
  settings: {} as Record<string, string>,
  intro: 0,
  ready: 0,
  finished: 0,
  endpoint: null as string | null,
  opens: true,
  material: "none" as Material,
}));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    settingGet: async (key: string) => native.settings[key] ?? null,
    settingSet: async (key: string, value: string) => {
      native.settings[key] = value;
      return { accepted: true, operation_id: null };
    },
    uiInfo: async () => ({ material: native.material }),
    operationsReconcile: async () => [],
    operationAcknowledge: async () => true,
    uiReady: async () => {
      native.ready++;
      return true;
    },
    onboardingFinish: async () => {
      native.finished++;
      return native.opens;
    },
    accountGet: async () =>
      ({
        account: { available: true, hosted_models: true, signed_in: false },
        fault: null,
        retry_after: null,
      }) as unknown as AccountReplyV1,
    workLocalServers: async () => [
      { kind: "ollama", base: "http://localhost:11434/v1", models: 3 },
    ],
    workSetModelEndpoint: async (_profile: string, base: string | null) => {
      native.endpoint = base;
      return {} as WorkModelsV1;
    },
    onboardingPlayIntro: async () => {
      native.intro++;
      return true;
    },
    keymapEntries: async () => [
      {
        id: "url.focus",
        title: "Open Location",
        group: "view",
        accelerator: "CmdOrCtrl+L",
        default_accelerator: "CmdOrCtrl+L",
        customizable: true,
        customized: false,
      },
    ],
    launcherTrigger: async () => ({
      shortcut: "CmdOrCtrl+Shift+Space",
      default_shortcut: "CmdOrCtrl+Shift+Space",
      registered: true,
      editable: true,
      double_tap: "off",
      double_tap_supported: true,
      accessibility: true,
    }),
  });
});
vi.mock("$domain/tabs", async () => ({
  tabs: (await import("./onboarding-tabs.svelte")).fakeTabs,
}));

let report: ((job: ImportJob) => void) | null = null;
const importer: ImportAdapter = {
  sources: async () => [
    {
      id: "arc",
      browser: "arc",
      name: "Arc",
      profiles: [{ id: "Default", name: "Personal" }],
      kinds: ["bookmarks", "history"],
      needsPermission: false,
      running: false,
    },
    {
      id: "chrome",
      browser: "chrome",
      name: "Chrome",
      profiles: [{ id: "Default", name: "Alex" }],
      kinds: ["bookmarks", "history"],
      needsPermission: false,
      running: false,
    },
    {
      id: "safari",
      browser: "safari",
      name: "Safari",
      profiles: [],
      kinds: ["bookmarks", "history"],
      needsPermission: true,
      running: false,
    },
  ],
  start: async () => true,
  cancel: async () => {},
  onProgress: (listener) => {
    report = listener;
    return () => (report = null);
  },
  openPermissionSettings: async () => {},
};

beforeEach(async () => {
  native.settings = {};
  native.intro = 0;
  native.ready = 0;
  native.finished = 0;
  native.endpoint = null;
  native.opens = true;
  native.material = "none";
  fakeTabs.reset();
  browserImport.provide(importer);
  await page.viewport(1320, 860);
});
afterEach(() => {
  browserImport.provide(null);
  delete document.documentElement.dataset.theme;
  delete document.documentElement.dataset.material;
  document.body.style.background = "";
});

const settled = () =>
  Promise.all(
    document
      .getAnimations()
      .filter((animation) => animation.effect?.getTiming().iterations !== Infinity)
      .map((animation) => animation.finished.catch(() => undefined)),
  );
const pause = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

async function capture(name: string, { wait = true } = {}) {
  if (wait) await Promise.race([settled(), pause(3000)]);
  for (const theme of ["dark", "light"]) {
    document.documentElement.dataset.theme = theme;
    await pause(80);
    await page.screenshot({ path: `../../../../../target/onboarding/${name}-${theme}.png` });
  }
  document.documentElement.dataset.theme = "dark";
}

const next = () =>
  page
    .getByRole("navigation")
    .getByRole("button", { name: /^(Continue|Skip|Not now)/u })
    .click();
/** The name has formed: only then does welcome answer. */
const formed = () =>
  expect
    .poll(() => document.querySelector(".onboarding")?.getAttribute("data-phase"), {
      timeout: 6000,
    })
    .toMatch(/^(formed|still)$/u);

test("the first run, from the name in the air to the browser", { timeout: 180_000 }, async () => {
  document.documentElement.dataset.theme = "dark";
  render(OnboardingApp);
  // Drawn before native is told it may show the window, and told once.
  await expect.poll(() => native.ready).toBe(1);

  // The air gathers into the name, once, with the intro asked for once.
  await pause(900);
  await capture("01-forming", { wait: false });
  const begin = page.getByRole("button", { name: /Get started/u });
  await formed();
  await pause(1600);
  await capture("02-welcome", { wait: false });
  expect(native.intro).toBe(1);
  // Nothing of the formation is left running.
  await expect.poll(() => document.querySelector("canvas.air"), { timeout: 4000 }).toBeNull();

  // Welcome pulls back from the new tab's sky to the window it belongs to.
  await begin.click();
  await expect
    .element(page.getByRole("heading", { name: "Bring your web with you." }))
    .toBeVisible();
  // No greeting is turned on, and the name is left as it was.
  expect(native.settings["ui.newtab-greeting"]).toBeUndefined();
  expect(fakeTabs.profile().name).toBe("Personal");
  // The engine chosen beside the import is the one search uses.
  await page.getByRole("radio", { name: "Google" }).click();
  await expect.poll(() => native.settings["search.engine"]).toBe("google");
  await page.getByRole("radio", { name: "Arc" }).click();
  await capture("04-import");
  await page.getByRole("button", { name: "Import", exact: true }).click();
  report?.({
    source: "arc",
    profile: "Default",
    finished: false,
    cancelled: false,
    kinds: [
      { kind: "bookmarks", state: "done", done: 1204, total: 1204 },
      { kind: "history", state: "running", done: 9200, total: 18392 },
    ],
  });
  await capture("05-importing");
  report?.({
    source: "arc",
    profile: "Default",
    finished: true,
    cancelled: false,
    kinds: [
      { kind: "bookmarks", state: "done", done: 1204, total: 1204 },
      { kind: "history", state: "done", done: 18392, total: 18392 },
    ],
  });
  await expect.element(page.getByText("19,596 items brought over")).toBeVisible();
  await capture("06-imported");

  await next();
  // Every site kept is thrown into the dock, not only the first.
  let thrown = 0;
  const watch = new MutationObserver((changes) => {
    for (const change of changes)
      for (const added of change.addedNodes)
        if (added instanceof HTMLElement && added.classList.contains("flight")) thrown++;
  });
  watch.observe(document.body, { childList: true, subtree: true });
  for (const site of ["Slack", "Notion", "Linear", "Figma", "GitHub"]) {
    await page.getByRole("button", { name: `Keep ${site}` }).click();
    await pause(160);
  }
  await expect
    .poll(() => fakeTabs.sidebarNodes().filter((node) => node.section === "favorites").length)
    .toBe(5);
  await expect.poll(() => thrown).toBe(5);
  watch.disconnect();
  await capture("07-essentials");
  // A second press lets the site go: closing an Essential would keep it.
  await page.getByRole("button", { name: "Remove GitHub" }).click();
  await expect.poll(() => fakeTabs.sidebarNodes().length).toBe(5);
  await expect.element(page.getByRole("button", { name: "Keep GitHub" })).toBeVisible();

  await next();
  await expect
    .element(page.getByRole("heading", { name: "Always a keystroke away." }))
    .toBeVisible();
  await expect.element(page.getByText("For a page’s address in Zephium.")).toBeVisible();
  await expect
    .element(page.getByLabelText(acceleratorKeys("CmdOrCtrl+L", IS_MAC).join(" "), { exact: true }))
    .toBeVisible();
  // It opens and types on its own, and steps aside for the real one.
  await pause(1800);
  await capture("08-launcher", { wait: false });
  emitNativeEvent("uiCommand", "launcher.presented");
  await expect.element(page.getByText("Try it now, from any app.")).toBeVisible();
  await expect
    .poll(() => document.querySelector(".launcher[data-away]")?.getAttribute("data-away"))
    .toBe("true");
  emitNativeEvent("uiCommand", "launcher.dismissed");
  await expect
    .poll(() => document.querySelector(".launcher[data-away]")?.getAttribute("data-away"))
    .toBe("false");

  await next();
  await pause(5600);
  await capture("10-work", { wait: false });

  await next();
  await expect
    .element(page.getByRole("heading", { name: "Choose how Work thinks." }))
    .toBeVisible();
  // Nothing is chosen for the person; a server already running here is named.
  await expect.element(page.getByRole("button", { name: /^Not now/u })).toBeVisible();
  await expect.element(page.getByText("Ollama is running")).toBeVisible();
  await page.getByRole("radio", { name: /A local model/u }).click();
  await expect.element(page.getByRole("button", { name: /^Continue/u })).toBeVisible();
  await capture("10b-ai");

  await next();
  await expect.element(page.getByRole("heading", { name: "Ready when you are." })).toBeVisible();
  await capture("11-ready");

  // The scene steps back, then native takes the window for the browser and
  // nothing of onboarding stays mounted.
  await page.getByRole("button", { name: /Start browsing/u }).click();
  await expect.poll(() => native.finished, { timeout: 4000 }).toBe(1);
  // The running server is connected here, and the browser opens on the new
  // tab the scene ended on, with no other tab opened for it.
  expect(native.endpoint).toBe("http://localhost:11434/v1");
  expect(fakeTabs.opened()).toBe(0);
  await expect.poll(() => document.querySelector(".onboarding")).toBeNull();
});

test(
  "if the browser cannot open, onboarding comes back and says so",
  { timeout: 30_000 },
  async () => {
    native.opens = false;
    document.documentElement.dataset.theme = "dark";
    render(OnboardingApp);
    await formed();
    await page.getByRole("button", { name: /Get started/u }).click();
    await page.getByRole("button", { name: "Skip setup" }).click();
    await expect.element(page.getByText("The browser could not open. Try again.")).toBeVisible();
    expect(native.finished).toBe(1);
    await expect
      .poll(() => document.querySelector(".onboarding")?.getAttribute("data-leaving"))
      .toBe("false");
  },
);

test("over a material, the window's own glass is the ground", { timeout: 30_000 }, async () => {
  document.documentElement.dataset.theme = "dark";
  native.material = "liquid_glass";
  document.body.style.background =
    "linear-gradient(115deg, #2b1c3a, #c2553f 38%, #e9a26b 52%, #2f6b5e 70%, #10222a)";
  render(OnboardingApp);
  await formed();
  await pause(1600);
  await capture("02-welcome-glass", { wait: false });
});

test("skipping finishes onboarding from anywhere", { timeout: 20_000 }, async () => {
  document.documentElement.dataset.theme = "dark";
  render(OnboardingApp);
  await formed();
  await page.getByRole("button", { name: /Get started/u }).click();
  await page.getByRole("button", { name: "Skip setup" }).click();
  await expect.poll(() => native.finished, { timeout: 4000 }).toBe(1);
  await expect.poll(() => document.querySelector(".onboarding")).toBeNull();
});

test("Enter moves on", { timeout: 20_000 }, async () => {
  document.documentElement.dataset.theme = "dark";
  render(OnboardingApp);
  await formed();
  await userEvent.keyboard("{Enter}");
  await expect
    .element(page.getByRole("heading", { name: "Bring your web with you." }))
    .toBeVisible();
  await userEvent.keyboard("{Enter}");
  await expect
    .element(page.getByRole("heading", { name: "Keep what you use every day." }))
    .toBeVisible();
});

test("whatever was chosen, the browser opens on a new tab", { timeout: 30_000 }, async () => {
  document.documentElement.dataset.theme = "dark";
  // A profile from before, left on a page.
  fakeTabs.showPage("https://example.com/");
  render(OnboardingApp);
  await formed();
  await page.getByRole("button", { name: /Get started/u }).click();
  await page.getByRole("button", { name: "Skip setup" }).click();
  await expect.poll(() => native.finished, { timeout: 4000 }).toBe(1);
  expect(fakeTabs.opened()).toBe(1);
});
