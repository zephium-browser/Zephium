import "$styles/global.css";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { commands, page, userEvent } from "vitest/browser";
import type { GpuReport, RendererPreference } from "$shared/ipc/bindings";
import * as notices from "$session/notice.svelte";
import GraphicsPage from "../components/sections/GraphicsPage.svelte";
import { environmentOverride, gpuReport, heldBack, steppedDown } from "./graphics-fixtures";

declare module "vitest/browser" {
  interface BrowserCommands {
    setForcedColors: (forcedColors: "active" | null) => Promise<void>;
  }
}

const native = vi.hoisted(() => ({
  report: null as unknown as GpuReport,
  initial: "automatic" as RendererPreference,
  saveFails: false,
  readFails: false,
  set: vi.fn(),
  retry: vi.fn(),
  relaunch: vi.fn(async () => true),
}));

vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    gpuReport: async () =>
      native.readFails
        ? { status: "error", error: "Graphics startup report is unavailable" }
        : { status: "ok", data: native.report },
    rendererPreferenceSet: async (preference) => {
      native.set(preference);
      if (native.saveFails)
        return { status: "error", error: "Renderer preferences are unavailable for this launch" };
      native.report = {
        ...native.report,
        preference,
        restart_required: preference !== native.initial,
      };
      return { status: "ok", data: native.report };
    },
    rendererRetryFaster: async () => {
      native.retry();
      native.report = { ...native.report, retry_requested: true, restart_required: true };
      return { status: "ok", data: native.report };
    },
    appRelaunch: native.relaunch,
  });
});

const tab = () =>
  userEvent.keyboard(navigator.platform.includes("Mac") ? "{Alt>}{Tab}{/Alt}" : "{Tab}");
const radio = (name: string) => page.getByRole("radio", { name: new RegExp(`^${name}`, "u") });

beforeEach(() => {
  native.report = gpuReport();
  native.initial = "automatic";
  native.saveFails = false;
  native.readFails = false;
  notices.dismiss(notices.notice()?.id ?? 0);
});
afterEach(() => vi.clearAllMocks());

test("the four modes are one labelled radio group with Automatic chosen", async () => {
  const screen = await render(GraphicsPage);
  const group = screen.getByRole("radiogroup", { name: "Rendering mode" });
  await expect.element(group).toBeVisible();
  await expect.element(radio("Automatic")).toHaveAttribute("aria-checked", "true");
  for (const name of ["GPU", "Compatibility", "Safe"])
    await expect.element(radio(name)).toHaveAttribute("aria-checked", "false");
  await expect
    .element(radio("Compatibility"))
    .toHaveAccessibleDescription(expect.stringContaining("shared memory"));
  await expect
    .element(group)
    .toHaveAccessibleDescription(expect.stringContaining("Running in GPU mode"));
  expect(screen.container.textContent).not.toContain("WEBKIT_");
  await expect.element(screen.getByText("AMD (Mesa RadeonSI)")).toBeVisible();
  await expect.element(screen.getByText("Wayland · Mutter")).toBeVisible();
  await expect.element(screen.getByText("GPU, chosen by Automatic")).toBeVisible();
});

test("keyboard moves through the modes, saves each and offers a restart only on request", async () => {
  const screen = await render(GraphicsPage);
  await expect.element(radio("Automatic")).toHaveAttribute("aria-checked", "true");
  await tab();
  await expect.element(radio("Automatic")).toHaveFocus();
  await userEvent.keyboard("{ArrowDown}");
  await expect.element(radio("GPU")).toHaveFocus();
  await userEvent.keyboard("{ArrowDown}");
  await expect.element(radio("Compatibility")).toHaveFocus();
  await expect.element(radio("Compatibility")).toHaveAttribute("aria-checked", "true");
  await vi.waitFor(() => expect(native.report.preference).toBe("compatibility"));
  await expect.element(screen.getByText("Restart Zephium to use Compatibility")).toBeVisible();
  await expect
    .element(screen.getByText("Until then, Zephium keeps running in GPU mode."))
    .toBeVisible();
  await expect
    .element(screen.getByRole("status").filter({ hasText: "Saved." }))
    .toHaveTextContent("Saved. It takes effect the next time Zephium starts.");
  expect(native.relaunch).not.toHaveBeenCalled();

  // One Tab leaves the group for the restart it now offers.
  await tab();
  await expect.element(screen.getByRole("button", { name: "Restart Zephium" })).toHaveFocus();
  await userEvent.keyboard("{Enter}");
  expect(native.relaunch).toHaveBeenCalledOnce();

  await radio("Automatic").click();
  await vi.waitFor(() => expect(native.report.preference).toBe("automatic"));
  expect(screen.getByRole("button", { name: "Restart Zephium" }).query()).toBeNull();
  await userEvent.keyboard("{End}");
  await expect.element(radio("Safe")).toHaveFocus();
  await userEvent.keyboard("{Home}");
  await expect.element(radio("Automatic")).toHaveFocus();
});

test("a restart that does not start says so", async () => {
  native.relaunch.mockResolvedValueOnce(false);
  native.report = gpuReport({ preference: "safe", restart_required: true });
  const screen = await render(GraphicsPage);
  await screen.getByRole("button", { name: "Restart Zephium" }).click();
  await expect.element(screen.getByText("Zephium couldn't restart. Try again.")).toBeVisible();
});

test("an environment override names its variable and keeps the saved choice", async () => {
  native.report = gpuReport({
    ...environmentOverride(),
    preference: "gpu",
    running_preference: "gpu",
  });
  native.initial = "gpu";
  const screen = await render(GraphicsPage);
  await expect
    .element(screen.getByText("Running in Safe mode, set by an environment variable"))
    .toBeVisible();
  await expect.element(screen.getByText("WEBKIT_DISABLE_DMABUF_RENDERER=1")).toBeVisible();
  expect(screen.getByText("GDK_BACKEND=wayland").query()).toBeNull();
  await expect.element(radio("GPU")).toHaveAttribute("aria-checked", "true");
  await radio("Automatic").click();
  await expect
    .element(screen.getByRole("status").filter({ hasText: "Saved." }))
    .toHaveTextContent("Saved. An environment variable still overrides it.");
  expect(screen.getByRole("button", { name: "Restart Zephium" }).query()).toBeNull();
});

test("after a failed start Zephium says it stepped down and can keep that mode", async () => {
  native.report = gpuReport(steppedDown());
  const screen = await render(GraphicsPage);
  await expect
    .element(screen.getByText("Running in Compatibility mode after a startup problem"))
    .toBeVisible();
  await expect.element(radio("Automatic")).toHaveAttribute("aria-checked", "true");
  await expect.element(screen.getByText("Compatibility, after a failed start")).toBeVisible();
  await screen.getByRole("button", { name: "Keep Compatibility Mode" }).click();
  expect(native.set).toHaveBeenCalledWith("compatibility");
  await expect.element(radio("Compatibility")).toHaveAttribute("aria-checked", "true");
  await expect
    .element(
      screen.getByText("Saved. Zephium already runs this way, so there’s no need to restart."),
    )
    .toBeVisible();
  expect(screen.getByRole("button", { name: "Restart Zephium" }).query()).toBeNull();
  // Automatic is one choice away, and the step-down explanation returns with it.
  await radio("Automatic").click();
  await expect
    .element(screen.getByText("Running in Compatibility mode after a startup problem"))
    .toBeVisible();
});

test("Automatic staying in a slower mode explains why and can try the faster one again", async () => {
  native.report = gpuReport(heldBack());
  const screen = await render(GraphicsPage);
  await expect
    .element(screen.getByText("Automatic is staying in Compatibility mode"))
    .toBeVisible();
  await expect
    .element(screen.getByText(/failed to start twice with this graphics driver/u))
    .toBeVisible();
  await expect.element(radio("Automatic")).toHaveAttribute("aria-checked", "true");
  await expect
    .element(screen.getByText("Compatibility, after repeated failed starts"))
    .toBeVisible();
  expect(screen.getByRole("button", { name: "Restart Zephium" }).query()).toBeNull();
  await screen.getByRole("button", { name: "Try the Faster Mode Again" }).click();
  expect(native.retry).toHaveBeenCalledOnce();
  expect(native.set).not.toHaveBeenCalled();
  await expect
    .element(screen.getByText("Restart Zephium to try the faster mode again"))
    .toBeVisible();
  await expect
    .element(screen.getByRole("status").filter({ hasText: "Saved." }))
    .toHaveTextContent("Saved. It takes effect the next time Zephium starts.");
  expect(native.relaunch).not.toHaveBeenCalled();
  await screen.getByRole("button", { name: "Restart Zephium" }).click();
  expect(native.relaunch).toHaveBeenCalledOnce();
});

test("details keep the native selection reason out of the page", async () => {
  native.report = gpuReport(steppedDown());
  const screen = await render(GraphicsPage);
  await expect.element(screen.getByText("Compatibility, after a failed start")).toBeVisible();
  expect(screen.container.textContent).not.toContain("previous automatic startup");
  expect(screen.container.textContent).not.toContain("fallback 1/2");
});

test("a choice that cannot be saved says so and shows what is stored", async () => {
  native.saveFails = true;
  const screen = await render(GraphicsPage);
  await expect.element(radio("Automatic")).toHaveAttribute("aria-checked", "true");
  await radio("Safe").click();
  await expect.element(screen.getByText("Couldn’t save your choice. Try again.")).toBeVisible();
  await expect.element(radio("Automatic")).toHaveAttribute("aria-checked", "true");
  await expect.element(radio("Safe")).toHaveAttribute("aria-checked", "false");
});

test("an unreadable report keeps the page's shape and can be tried again", async () => {
  native.readFails = true;
  const screen = await render(GraphicsPage);
  await expect.element(screen.getByText("Graphics information is unavailable")).toBeVisible();
  await expect.element(screen.getByRole("radiogroup")).toHaveAttribute("aria-disabled", "true");
  (radio("Safe").element() as HTMLElement).click();
  expect(native.set).not.toHaveBeenCalled();
  await expect.element(screen.getByRole("button", { name: "Copy Report" })).toBeDisabled();
  await expect.element(screen.getByText("Driver", { exact: true })).toBeVisible();
  native.readFails = false;
  await screen.getByRole("button", { name: "Try again" }).click();
  await expect.element(screen.getByText("AMD (Mesa RadeonSI)")).toBeVisible();
  await expect.element(screen.getByRole("radiogroup")).not.toHaveAttribute("aria-disabled");
});

test("Copy Report copies the full plain-text report and confirms it", async () => {
  const write = vi
    .spyOn(navigator.clipboard, "write")
    .mockImplementation(async (items: ClipboardItems) => {
      await items[0]?.getType("text/plain");
    });
  const screen = await render(GraphicsPage);
  await expect.element(screen.getByText("AMD (Mesa RadeonSI)")).toBeVisible();
  await screen.getByRole("button", { name: "Copy Report" }).click();
  await vi.waitFor(() => expect(write).toHaveBeenCalledOnce());
  const item = write.mock.calls[0]?.[0][0];
  expect(await (await item?.getType("text/plain"))?.text()).toBe(native.report.text);
  await vi.waitFor(() => expect(notices.notice()?.text).toBe("Graphics report copied"));

  await screen.getByRole("button", { name: "Show What Is Copied" }).click();
  await expect
    .element(screen.getByRole("textbox", { name: "Graphics report" }))
    .toHaveValue(native.report.text);
});

test("a clipboard that refuses everything leaves the report to copy by hand", async () => {
  vi.spyOn(navigator.clipboard, "write").mockRejectedValue(
    new DOMException("no", "NotAllowedError"),
  );
  vi.spyOn(navigator.clipboard, "writeText").mockRejectedValue(
    new DOMException("no", "NotAllowedError"),
  );
  const screen = await render(GraphicsPage);
  await expect.element(screen.getByText("AMD (Mesa RadeonSI)")).toBeVisible();
  await screen.getByRole("button", { name: "Copy Report" }).click();
  await expect
    .element(screen.getByText("Couldn’t copy. Select the text below and copy it yourself."))
    .toBeVisible();
  await expect
    .element(screen.getByRole("textbox", { name: "Graphics report" }))
    .toHaveValue(native.report.text);
  expect(notices.notice()).toBeNull();
});

test("native warnings are marked as untranslated English", async () => {
  native.report = gpuReport({ state_warnings: ["renderer state could not be read"] });
  const screen = await render(GraphicsPage);
  const warning = screen.getByText("renderer state could not be read");
  await expect.element(warning).toHaveAttribute("lang", "en");
  await expect.element(screen.getByRole("list", { name: "Warnings" })).toBeVisible();
});

test("forced colours draw the chosen mode in the system highlight", async ({ onTestFinished }) => {
  onTestFinished(() => commands.setForcedColors(null));
  await commands.setForcedColors("active");
  native.report = gpuReport({ preference: "safe", restart_required: true });
  const screen = await render(GraphicsPage);
  await expect.element(radio("Safe")).toHaveAttribute("aria-checked", "true");
  const dot = (name: string) => radio(name).element().querySelector(".dot")!;
  const other = getComputedStyle(dot("GPU"));
  expect(other.borderTopStyle).toBe("solid");
  await expect
    .poll(() => getComputedStyle(dot("Safe")).backgroundColor)
    .not.toBe(other.backgroundColor);
  // The system's text backplate would hide a forced primary label.
  const restart = screen.getByRole("button", { name: "Restart Zephium" }).element();
  expect(getComputedStyle(restart).forcedColorAdjust).toBe("none");
  expect(getComputedStyle(restart).color).not.toBe(getComputedStyle(restart).backgroundColor);
});

test("Reduce motion stills the radio", async () => {
  document.documentElement.dataset.reduceMotion = "true";
  try {
    await render(GraphicsPage);
    const dot = radio("Automatic").element().querySelector(".dot")!;
    expect(Number.parseFloat(getComputedStyle(dot, "::after").transitionDuration)).toBeLessThan(
      0.001,
    );
  } finally {
    delete document.documentElement.dataset.reduceMotion;
  }
});
