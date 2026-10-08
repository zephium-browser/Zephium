import { afterEach, expect, test, vi } from "vitest";
import { page } from "vitest/browser";
import { render } from "vitest-browser-svelte";
import type { DownloadCall, UpdateStatus } from "$shared/ipc/bindings";
import { emitNativeEvent } from "$shared/testing/native-events";
import { runtime } from "$domain/runtime";
import { updates } from "$domain/updates";
import { updateNotices } from "$features/updates";
import UpdateLook from "./UpdateLook.svelte";

const native = vi.hoisted(() => ({
  status: { state: "ready", version: "1.0.2" } as UpdateStatus,
  stored: { "notice.seen-version": "1.0.0" } as Record<string, string>,
}));

const cleanup = { running: false, error: null };
const receiving = {
  id: "d1",
  revision: "1",
  created_at: "2026-10-04T10:00:00Z",
  filename: "Zephium-1.0.2.dmg",
  source: "https://example.com",
  state: "receiving" as const,
  received: "48000000",
  total: "120000000",
  error: null,
};

vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    updateStatus: async () => native.status,
    aboutInfo: async () => ({ version: "1.0.1", os: "macOS 26.1.0", arch: "aarch64" }),
    settingGet: async (key: string) => native.stored[key] ?? null,
    settingSet: async () => ({ accepted: true, operation_id: null }),
    downloadCall: async (_profile: string, call: DownloadCall) =>
      call.kind === "updates"
        ? { kind: "updates" as const, entries: [receiving], removed: [], cleanup }
        : { kind: "page" as const, entries: [receiving], next: null, supported: true, cleanup },
    runCommand: async () => ({ accepted: true, operation_id: null }),
    toolsMenuPopup: async () => true,
    updateHighlights: async () => ({
      version: "1.0.1",
      items: [
        "Links to Zoom, Teams and Slack open in their apps",
        "Page dialogs show instead of answering for you",
        "Private windows close from one place",
      ],
    }),
  });
});

afterEach(() => {
  updateNotices.dispose();
  updates.dispose();
  runtime.dispose();
});

async function start(advisory: boolean) {
  await runtime.init();
  emitNativeEvent("runtimeStatusChanged", {
    restart_required: false,
    session_set_aside: false,
    user_content_degraded_scope_count: 0,
    security_advisories: advisory
      ? [{ kind: "update_recommended", update_target: "operating_system" }]
      : [],
  });
  await updates.init();
  await updateNotices.init();
}

async function shoot(target: HTMLElement, name: string) {
  for (const theme of ["dark", "light"]) {
    document.documentElement.dataset.theme = theme;
    await page.elementLocator(target).screenshot({
      path: `../../../../../target/update-ui/${name}-${theme}.png`,
    });
  }
  document.documentElement.dataset.theme = "dark";
}

// Rendered references for the foot of the column, in both themes, beside a
// download in progress and at rail width.
test("the card and the pill sit between a download and the dock", async () => {
  await start(false);
  await page.viewport(420, 520);
  const screen = await render(UpdateLook, { props: { download: true } });
  await expect.element(screen.getByText("Zephium updated to 1.0.1")).toBeVisible();
  await expect
    .element(screen.getByText("Page dialogs show instead of answering for you"))
    .toBeVisible();
  await expect.element(screen.getByText("Something broke? Tell us")).toBeVisible();
  await expect.element(screen.getByText("Zephium-1.0.2.dmg")).toBeVisible();
  const stack = screen.container.querySelector<HTMLElement>(".update-stack")!;
  const card = stack.querySelector("[data-variant='card']")!;
  const pill = stack.querySelector("[data-variant='pill']")!;
  const dock = screen.container.querySelector(".dock")!;
  const download = screen.container.querySelector(".download-card")!;
  // Download, then card, then pill, then dock, none overlapping.
  const bottom = (element: Element) => element.getBoundingClientRect().bottom;
  const top = (element: Element) => element.getBoundingClientRect().top;
  expect(bottom(download)).toBeLessThanOrEqual(top(card));
  expect(bottom(card)).toBeLessThanOrEqual(top(pill));
  expect(bottom(pill)).toBeLessThanOrEqual(top(dock));
  expect(download.getBoundingClientRect().width).toBe(card.getBoundingClientRect().width);
  expect(card.getBoundingClientRect().width).toBe(pill.getBoundingClientRect().width);
  await new Promise((done) => setTimeout(done, 400));
  await shoot(screen.container, "column");
});

test("the security card at full width", async () => {
  await start(true);
  await page.viewport(420, 520);
  const screen = await render(UpdateLook);
  await expect.element(screen.getByRole("button", { name: "Dismiss" })).toBeVisible();
  await new Promise((done) => setTimeout(done, 400));
  await shoot(screen.container, "security");
});

test("one glyph at rail width", async () => {
  await start(false);
  await page.viewport(420, 520);
  const screen = await render(UpdateLook, { props: { compact: true } });
  const plate = screen.getByRole("button", { name: "Relaunch to update" });
  await expect.element(plate).toBeVisible();
  expect(plate.element().getBoundingClientRect().width).toBe(40);
  await new Promise((done) => setTimeout(done, 400));
  await shoot(screen.container, "rail");
});
