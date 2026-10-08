import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import "$styles/global.css";
import type { RuntimeStatus, UpdateStatus } from "$shared/ipc/bindings";
import { emitNativeEvent } from "$shared/testing/native-events";
import { IS_MAC } from "$shared/platform";
import { runtime } from "$domain/runtime";
import { updates } from "$domain/updates";
import { updateNotices } from "..";
import UpdateCards from "../components/UpdateCards.svelte";
import UpdateGlyph from "../components/UpdateGlyph.svelte";

const native = vi.hoisted(() => ({
  status: { state: "upToDate" } as UpdateStatus,
  stored: {} as Record<string, string>,
  about: vi.fn(async () => ({ version: "1.0.1", os: "macOS 26.1.0", arch: "aarch64" })),
  relaunch: vi.fn(async () => true),
  openUrl: vi.fn(async () => ({ accepted: true, operation_id: null })),
  softwareUpdate: vi.fn(async () => true),
  settingSet: vi.fn(async (_key: string, _value: string) => ({
    accepted: true,
    operation_id: null,
  })),
}));

vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    updateStatus: async () => native.status,
    updateRelaunch: native.relaunch,
    aboutInfo: native.about,
    settingGet: async (key: string) => native.stored[key] ?? null,
    settingSet: native.settingSet,
    browserOpenUrl: native.openUrl,
    openSoftwareUpdate: native.softwareUpdate,
  });
});

const clear: RuntimeStatus = {
  restart_required: false,
  session_set_aside: false,
  user_content_degraded_scope_count: 0,
  security_advisories: [],
};

async function start(
  status: UpdateStatus,
  stored: Record<string, string>,
  advisories: RuntimeStatus["security_advisories"] = [],
) {
  native.status = status;
  native.stored = stored;
  await runtime.init();
  emitNativeEvent("runtimeStatusChanged", { ...clear, security_advisories: advisories });
  await updates.init();
  await updateNotices.init();
}

beforeEach(() => {
  vi.clearAllMocks();
});
afterEach(() => {
  updateNotices.dispose();
  updates.dispose();
  runtime.dispose();
});

test("a first run only marks the version it started at", async () => {
  await start({ state: "upToDate" }, {});
  const screen = await render(UpdateCards);
  expect(native.settingSet).toHaveBeenCalledWith("notice.seen-version", "1.0.1");
  expect(screen.container.querySelector(".update-stack")).toBeNull();
});

test("after an update, the card links its release notes and goes once read", async () => {
  await start({ state: "upToDate" }, { "notice.seen-version": "1.0.0" });
  const screen = await render(UpdateCards);
  await expect.element(screen.getByText("Zephium updated to 1.0.1")).toBeVisible();

  await screen.getByRole("button", { name: "See what's new" }).click();
  expect(native.openUrl).toHaveBeenCalledWith(
    "https://github.com/zephium-browser/Zephium/releases/tag/v1.0.1",
    true,
  );
  await expect.poll(() => screen.container.querySelector(".update-stack")).toBeNull();
  expect(native.settingSet).toHaveBeenCalledWith("notice.seen-version", "1.0.1");
});

test("the close button acknowledges the new version without opening anything", async () => {
  await start({ state: "upToDate" }, { "notice.seen-version": "1.0.0" });
  const screen = await render(UpdateCards);
  await screen.getByRole("button", { name: "Dismiss" }).click();
  await expect.poll(() => screen.container.querySelector(".update-stack")).toBeNull();
  expect(native.settingSet).toHaveBeenCalledWith("notice.seen-version", "1.0.1");
  expect(native.openUrl).not.toHaveBeenCalled();
});

test("a platform security update comes first and returns only with a newer Zephium", async () => {
  await start({ state: "upToDate" }, { "notice.seen-version": "1.0.0" }, [
    { kind: "review_overdue", update_target: "zephium" },
    { kind: "update_recommended", update_target: "operating_system" },
  ]);
  const screen = await render(UpdateCards);
  const title = IS_MAC ? "macOS has security updates" : "Your system has security updates";
  await expect.element(screen.getByText(title)).toBeVisible();
  await expect.element(screen.getByText("Install them to keep browsing safely.")).toBeVisible();
  expect(screen.container.textContent).not.toContain("Zephium updated");
  expect(screen.container.textContent).not.toContain("review");

  if (IS_MAC) {
    await screen.getByRole("button", { name: "Open Software Update" }).click();
    expect(native.softwareUpdate).toHaveBeenCalledOnce();
    await expect.element(screen.getByText(title)).toBeVisible();
  }

  await screen.getByRole("button", { name: "Dismiss" }).click();
  await expect
    .poll(() => native.settingSet.mock.calls)
    .toContainEqual(["notice.security-dismissed", "1.0.1"]);
  await expect.element(screen.getByText("Zephium updated to 1.0.1")).toBeVisible();
});

test("WebView2 is named, with no action Zephium cannot take", async () => {
  await start({ state: "upToDate" }, { "notice.seen-version": "1.0.1" }, [
    { kind: "update_recommended", update_target: "browser_runtime" },
  ]);
  const screen = await render(UpdateCards);
  await expect.element(screen.getByText("WebView2 needs an update")).toBeVisible();
  expect(screen.container.querySelectorAll(".action")).toHaveLength(0);
});

test("a dismissed security notice stays away at the same version", async () => {
  await start(
    { state: "upToDate" },
    { "notice.seen-version": "1.0.1", "notice.security-dismissed": "1.0.1" },
    [{ kind: "update_recommended", update_target: "operating_system" }],
  );
  const screen = await render(UpdateCards);
  expect(screen.container.querySelector(".update-stack")).toBeNull();
});

test("internal review notices never reach the sidebar", async () => {
  await start({ state: "upToDate" }, { "notice.seen-version": "1.0.1" }, [
    { kind: "review_overdue", update_target: "zephium" },
    { kind: "unreviewed_runtime", update_target: "zephium" },
  ]);
  const screen = await render(UpdateCards);
  expect(screen.container.querySelector(".update-stack")).toBeNull();
});

test("a waiting update is one pill that relaunches, then shows it is under way", async () => {
  await start({ state: "ready", version: "1.0.2" }, { "notice.seen-version": "1.0.1" });
  const screen = await render(UpdateCards);
  const pill = screen.getByRole("button", { name: "Relaunch to update" });
  await expect.element(pill).toBeVisible();
  expect(screen.getByRole("button", { name: "Dismiss" }).query()).toBeNull();

  native.status = { state: "installing" };
  await pill.click();
  expect(native.relaunch).toHaveBeenCalledOnce();
  const pending = screen.getByRole("button", { name: "Updating…" });
  await expect.element(pending).toBeDisabled();
  await expect.element(pending).toHaveAttribute("aria-busy", "true");
});

test("a development build shows nothing at all", async () => {
  await start({ state: "unavailable" }, { "notice.seen-version": "0.9.0" }, [
    { kind: "update_recommended", update_target: "operating_system" },
  ]);
  const screen = await render(UpdateCards);
  const rail = await render(UpdateGlyph, { props: { onabout: () => {} } });
  expect(screen.container.querySelector(".update-stack")).toBeNull();
  expect(rail.container.querySelector(".update-plate")).toBeNull();
});

test("the rail's one glyph relaunches an update, or hands a notice to About", async () => {
  await start({ state: "upToDate" }, { "notice.seen-version": "1.0.0" });
  const onabout = vi.fn();
  const screen = await render(UpdateGlyph, { props: { onabout } });
  const plate = screen.getByRole("button", { name: "Zephium updated to 1.0.1" });
  await expect.element(plate).toHaveAttribute("title", "Zephium updated to 1.0.1");
  await plate.click();
  expect(onabout).toHaveBeenCalledOnce();
  expect(native.settingSet).toHaveBeenCalledWith("notice.seen-version", "1.0.1");
  await expect.poll(() => screen.container.querySelector(".update-plate")).toBeNull();

  native.status = { state: "ready", version: "1.0.2" };
  await updates.refresh();
  await screen.getByRole("button", { name: "Relaunch to update" }).click();
  expect(native.relaunch).toHaveBeenCalledOnce();
  expect(onabout).toHaveBeenCalledOnce();
});

test("manual installation remains actionable in the sidebar and compact rail", async () => {
  await start({ state: "manualInstall", version: "1.0.2" }, { "notice.seen-version": "1.0.1" });
  const screen = await render(UpdateCards);
  await screen.getByRole("button", { name: "Download and install manually" }).click();
  expect(native.openUrl).toHaveBeenCalledWith(
    "https://github.com/zephium-browser/Zephium/releases/tag/v1.0.2",
    true,
  );
  await screen.unmount();
  const rail = await render(UpdateGlyph, { props: { onabout: vi.fn() } });
  await rail.getByRole("button", { name: "Download and install manually" }).click();
  expect(native.openUrl).toHaveBeenCalledTimes(2);
  expect(native.relaunch).not.toHaveBeenCalled();
});
