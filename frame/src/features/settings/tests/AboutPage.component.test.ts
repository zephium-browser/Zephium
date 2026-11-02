import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import type { UpdateStatus } from "$shared/ipc/bindings";
import { emitNativeEvent } from "$shared/testing/native-events";
import { IS_MAC } from "$shared/platform";
import { preferences } from "$domain/preferences";
import { runtime } from "$domain/runtime";
import { updates } from "$domain/updates";
import * as notices from "$session/notice.svelte";
import { searchSettings } from "../lib/settings-model";
import AboutPage from "../components/sections/AboutPage.svelte";

const REPORT = vi.hoisted(() => "Zephium diagnostics\nApp: Zephium 1.0.1\nLog: 2 lines");

const native = vi.hoisted(() => ({
  status: { state: "upToDate" } as UpdateStatus,
  check: vi.fn(async (): Promise<UpdateStatus> => ({ state: "ready", version: "1.0.2" })),
  relaunch: vi.fn(async () => true),
  openUrl: vi.fn(async () => ({ accepted: true, operation_id: null })),
  softwareUpdate: vi.fn(async () => true),
  report: vi.fn(async (): Promise<string | null> => REPORT),
  logs: vi.fn(async () => true),
}));

vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    aboutInfo: async () => ({ version: "1.0.1", os: "macOS 26.1.0", arch: "aarch64" }),
    updateStatus: async () => native.status,
    updateCheck: native.check,
    updateRelaunch: native.relaunch,
    browserOpenUrl: native.openUrl,
    openSoftwareUpdate: native.softwareUpdate,
    diagnosticsReport: native.report,
    diagnosticsShowLogs: native.logs,
  });
});

vi.mock("$domain/preferences", () => ({
  preferences: {
    value: (key: string) => ({ "updates.auto-check": "true" })[key],
    aiEnabled: () => true,
    enabled: () => true,
    saving: () => false,
    saveFailed: () => false,
    set: vi.fn().mockResolvedValue(undefined),
  },
}));

beforeEach(async () => {
  vi.clearAllMocks();
  await runtime.init();
});
afterEach(() => {
  updates.dispose();
  runtime.dispose();
});

test("updates are checked, downloaded and relaunched from About", async () => {
  native.status = { state: "upToDate" };
  const screen = await render(AboutPage);
  await expect.element(screen.getByText("Zephium is up to date")).toBeVisible();
  await expect.element(screen.getByText("Version 1.0.1")).toBeVisible();

  await screen.getByRole("button", { name: "Check now" }).click();
  expect(native.check).toHaveBeenCalledOnce();
  await expect.element(screen.getByText("Version 1.0.2 is ready")).toBeVisible();

  native.status = { state: "installing" };
  await screen.getByRole("button", { name: "Relaunch to update" }).click();
  expect(native.relaunch).toHaveBeenCalledOnce();
  await expect.element(screen.getByText("Updating…")).toBeVisible();

  await screen.getByRole("switch", { name: "Check for updates automatically" }).click();
  expect(preferences.set).toHaveBeenCalledWith("updates.auto-check", "false");

  await screen.getByRole("button", { name: "Release Notes" }).click();
  expect(native.openUrl).toHaveBeenCalledWith("https://zephium.app/changelog#v1-0-1", true);
});

test("a failed check says so and can be tried again", async () => {
  native.status = { state: "idle" };
  native.check.mockResolvedValueOnce({ state: "failed" });
  const screen = await render(AboutPage);
  await screen.getByRole("button", { name: "Check now" }).click();
  await expect
    .element(
      screen.getByText("Couldn't complete the update. Try again, or download the latest version."),
    )
    .toBeVisible();
  await expect.element(screen.getByRole("button", { name: "Check now" })).toBeEnabled();
});

test("a development build only says that updates are off", async () => {
  native.status = { state: "unavailable" };
  const screen = await render(AboutPage);
  await expect.element(screen.getByText("Updates are off in development builds")).toBeVisible();
  expect(screen.getByRole("button", { name: "Check now" }).query()).toBeNull();
  expect(
    screen.getByRole("switch", { name: "Check for updates automatically" }).query(),
  ).toBeNull();
  expect(screen.getByRole("button", { name: "Release Notes" }).query()).toBeNull();
});

test("a system behind on security updates is explained where the rail sends people", async () => {
  native.status = { state: "upToDate" };
  emitNativeEvent("runtimeStatusChanged", {
    restart_required: false,
    session_set_aside: false,
    user_content_degraded_scope_count: 0,
    security_advisories: [
      { kind: "review_overdue", update_target: "zephium" },
      { kind: "update_recommended", update_target: "operating_system" },
    ],
  });
  const screen = await render(AboutPage);
  await expect.element(screen.getByText("Install them to keep browsing safely.")).toBeVisible();
  expect(screen.container.textContent).not.toContain("review");
  if (IS_MAC) {
    await screen.getByRole("button", { name: "Open Software Update" }).click();
    expect(native.softwareUpdate).toHaveBeenCalledOnce();
  }
});

test("settings search finds the update controls", () => {
  expect(searchSettings("updates automatically").map((result) => result.target)).toContain(
    "updates.auto-check",
  );
  expect(searchSettings("new version").map((result) => result.target)).toContain("about.updates");
});

test("manual installation opens the exact release instead of retrying an impossible swap", async () => {
  native.status = { state: "manualInstall", version: "1.0.2" };
  const screen = await render(AboutPage);
  await screen.getByRole("button", { name: "Download and install manually" }).click();
  expect(native.openUrl).toHaveBeenCalledWith("https://zephium.app/changelog#v1-0-2", true);
  expect(screen.getByRole("button", { name: "Relaunch to update" }).query()).toBeNull();
});

test("a postponed update remains ready with its failure explained", async () => {
  native.status = {
    state: "ready",
    version: "1.0.2",
    retry_reason:
      "The previous update did not finish. Your downloaded update is still ready to install.",
  };
  const screen = await render(AboutPage);
  await expect.element(screen.getByText(native.status.retry_reason!)).toBeVisible();
  await expect.element(screen.getByRole("button", { name: "Relaunch to update" })).toBeEnabled();
});

test("the links open Zephium's own pages in a tab", async () => {
  native.status = { state: "upToDate" };
  const screen = await render(AboutPage);
  const links = screen.getByRole("navigation", { name: "Zephium on the web" });
  for (const [name, url] of [
    ["Website", "https://zephium.app"],
    [IS_MAC ? "Docs" : "Help", "https://zephium.app/docs"],
    ["GitHub", "https://github.com/zephium-browser/Zephium"],
    ["Discord", "https://discord.gg/tyveTUyEp7"],
    ["YouTube", "https://www.youtube.com/@crynta"],
  ] as const) {
    await links.getByRole("link", { name }).click();
    expect(native.openUrl).toHaveBeenLastCalledWith(url, true);
  }
});

/** What the clipboard does: the item's text is awaited, and a failure there rejects the write. */
async function acceptingWrite(items: ClipboardItems) {
  await items[0]?.getType("text/plain");
}

test("one click copies the diagnostics, and the preview shows the same text", async () => {
  native.status = { state: "upToDate" };
  notices.dismiss(notices.notice()?.id ?? 0);
  const write = vi.spyOn(navigator.clipboard, "write").mockImplementation(acceptingWrite);
  const screen = await render(AboutPage);

  await screen.getByRole("button", { name: "Show What Is Copied" }).click();
  await expect
    .element(screen.getByRole("textbox", { name: "Diagnostic information" }))
    .toHaveValue(REPORT);

  await screen.getByRole("button", { name: "Copy Diagnostics" }).click();
  await vi.waitFor(() => expect(write).toHaveBeenCalledOnce());
  const item = write.mock.calls[0]?.[0][0];
  expect(await (await item?.getType("text/plain"))?.text()).toBe(REPORT);
  await vi.waitFor(() =>
    expect(notices.notice()?.text).toBe("Copied. Paste it into your GitHub issue."),
  );
  expect(native.report).toHaveBeenCalledTimes(2);

  await screen.getByRole("button", { name: "Hide" }).click();
  expect(screen.getByRole("textbox", { name: "Diagnostic information" }).query()).toBeNull();
});

test("a clipboard that refuses the rich write is tried again as text", async () => {
  native.status = { state: "upToDate" };
  notices.dismiss(notices.notice()?.id ?? 0);
  vi.spyOn(navigator.clipboard, "write").mockRejectedValue(
    new DOMException("no", "NotAllowedError"),
  );
  const writeText = vi.spyOn(navigator.clipboard, "writeText").mockResolvedValue();
  const screen = await render(AboutPage);
  await screen.getByRole("button", { name: "Copy Diagnostics" }).click();
  await vi.waitFor(() => expect(writeText).toHaveBeenCalledWith(REPORT));
  await vi.waitFor(() => expect(notices.notice()?.text).toContain("Copied."));
});

test("a clipboard that refuses everything leaves the text to copy by hand", async () => {
  native.status = { state: "upToDate" };
  notices.dismiss(notices.notice()?.id ?? 0);
  vi.spyOn(navigator.clipboard, "write").mockRejectedValue(
    new DOMException("no", "NotAllowedError"),
  );
  vi.spyOn(navigator.clipboard, "writeText").mockRejectedValue(
    new DOMException("no", "NotAllowedError"),
  );
  const screen = await render(AboutPage);
  await screen.getByRole("button", { name: "Copy Diagnostics" }).click();
  await expect
    .element(screen.getByText("Couldn’t copy. Select the text below and copy it yourself."))
    .toBeVisible();
  await expect
    .element(screen.getByRole("textbox", { name: "Diagnostic information" }))
    .toHaveValue(REPORT);
  expect(notices.notice()).toBeNull();
});

test("diagnostics that cannot be read say so and can be tried again", async () => {
  native.status = { state: "upToDate" };
  notices.dismiss(notices.notice()?.id ?? 0);
  native.report.mockResolvedValueOnce(null);
  vi.spyOn(navigator.clipboard, "write").mockImplementation(acceptingWrite);
  const screen = await render(AboutPage);
  await screen.getByRole("button", { name: "Copy Diagnostics" }).click();
  await expect.element(screen.getByText("Couldn’t read the diagnostics. Try again.")).toBeVisible();
  expect(notices.notice()).toBeNull();
  await screen.getByRole("button", { name: "Copy Diagnostics" }).click();
  await vi.waitFor(() => expect(notices.notice()?.text).toContain("Copied."));
  expect(screen.getByText("Couldn’t read the diagnostics. Try again.").query()).toBeNull();
});

test("the log folder opens from About, and settings search finds both actions", async () => {
  native.status = { state: "upToDate" };
  const screen = await render(AboutPage);
  await screen.getByRole("button", { name: "Show Log Folder" }).click();
  expect(native.logs).toHaveBeenCalledOnce();
  expect(searchSettings("bug report").map((result) => result.target)).toContain(
    "about.diagnostics",
  );
});
