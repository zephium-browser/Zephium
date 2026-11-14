import "$styles/global.css";
import { test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { commands, page } from "vitest/browser";
import type { GpuReport } from "$shared/ipc/bindings";
import { accountFixture } from "$shared/testing/account";
import * as settings from "../lib/settings-state.svelte";
import SettingsLookStage from "./SettingsLookStage.svelte";
import { environmentOverride, gpuReport, heldBack, steppedDown } from "./graphics-fixtures";

declare module "vitest/browser" {
  interface BrowserCommands {
    setForcedColors: (forcedColors: "active" | null) => Promise<void>;
  }
}

const native = vi.hoisted(() => ({ report: null as GpuReport | null, unreadable: false }));
vi.mock("$shared/platform", async (original) => ({
  ...(await original<typeof import("$shared/platform")>()),
  IS_LINUX: true,
  IS_MAC: false,
  IS_WINDOWS: false,
}));
// A look bench, not a contract test: commands the pages read in passing
// answer nothing, and the ones the shot depends on answer for real.
vi.mock("$shared/ipc/bindings", async () => {
  const known: Record<string, (...args: never[]) => Promise<unknown>> = {
    accountGet: async () => ({ account: accountFixture(), fault: null, retry_after: null }),
    settingGet: async () => null,
    gpuReport: async () =>
      native.unreadable
        ? { status: "error", error: "Graphics startup report is unavailable" }
        : { status: "ok", data: native.report },
  };
  return {
    commands: new Proxy(known, {
      get: (target, key) =>
        typeof key === "string" ? (target[key] ?? (async () => null)) : undefined,
    }),
  };
});

const shots = "../../../../../target/look/graphics";
const settle = (ms = 450) => new Promise((done) => setTimeout(done, ms));

async function shoot(name: string, report: GpuReport | null, themes = ["dark", "light"]) {
  native.report = report;
  native.unreadable = report === null;
  const screen = await render(SettingsLookStage);
  settings.select("graphics");
  await settle();
  for (const theme of themes) {
    document.documentElement.dataset.theme = theme;
    await settle(250);
    await page.screenshot({ path: `${shots}/${name}-${theme}.png` });
  }
  document.documentElement.dataset.theme = "dark";
  await screen.unmount();
}

test("Graphics in each state it can explain, both themes", async () => {
  await page.viewport(1280, 1400);
  await shoot("automatic", gpuReport());
  await shoot(
    "automatic-adjusted",
    gpuReport({
      detection: { driver_class: "virtio_virgl" },
      selection: {
        path: "disable_dmabuf",
        reason:
          "virtio/virgl: DMA-BUF icon flicker observed with WebKitGTK 2.54.2; shared-memory acceptance pending",
      },
    }),
  );
  await shoot(
    "chosen-safe",
    gpuReport({
      preference: "safe",
      running_preference: "safe",
      selection: {
        path: "disable_dmabuf",
        source: "preference",
        reason: "renderer preference applies until changed and restarted",
      },
    }),
  );
  await shoot(
    "restart-pending",
    gpuReport({ preference: "compatibility", restart_required: true }),
  );
  await shoot("environment", gpuReport({ ...environmentOverride(), preference: "gpu" }));
  await shoot("step-down", gpuReport(steppedDown()));
  await shoot("held", gpuReport(heldBack()));
  await shoot(
    "held-retry-pending",
    gpuReport({ ...heldBack(), retry_requested: true, restart_required: true }),
  );
  const software = gpuReport({
    detection: { driver_class: "software", probe_outcome: "inconclusive" },
    state_warnings: ["renderer state directory is not private; preferences are read-only"],
  });
  software.detection.gl = {
    ...software.detection.gl!,
    renderer: "llvmpipe (LLVM 20.1.8, 256 bits)",
    software: true,
  };
  await shoot("software-warnings", software);
  await shoot("unavailable", null);
});

test("Graphics at a narrow width", async () => {
  await page.viewport(640, 1600);
  await shoot("narrow", gpuReport(steppedDown()));
});

test("Graphics in forced colours", async ({ onTestFinished }) => {
  onTestFinished(() => commands.setForcedColors(null));
  await commands.setForcedColors("active");
  await page.viewport(1280, 1400);
  await shoot("forced-colors", gpuReport({ preference: "safe", restart_required: true }), ["dark"]);
});
