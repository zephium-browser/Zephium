import { expect, test, vi } from "vitest";
import { page } from "vitest/browser";
import { render } from "vitest-browser-svelte";
import type { DownloadCall, DownloadView } from "$shared/ipc/bindings";
import "$styles/global.css";
import DownloadStatus from "../components/DownloadStatus.svelte";

const native = vi.hoisted(() => ({ call: vi.fn() }));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({ downloadCall: native.call });
});
vi.mock("$shared/ipc/native-events", () => ({
  events: { downloadsChanged: { listen: () => Promise.resolve(() => {}) } },
}));

const profile = "00000000000000000000000001";
const entry: DownloadView = {
  id: "00000000000000000000000009",
  revision: "00000001",
  created_at: "1",
  filename: "setup.exe",
  source: "https://example.com",
  source_is_context: false,
  state: "receiving",
  received: "10",
  total: "100",
  error: null,
};

function serving(initial: DownloadView, next: (call: DownloadCall) => DownloadView | null) {
  let current = initial;
  native.call.mockReset();
  native.call.mockImplementation(async (_profile: string, call: DownloadCall) => {
    if (call.kind === "updates")
      return {
        kind: "updates",
        cleanup: { running: false, error: null },
        entries: [current],
        removed: [],
      };
    if (call.kind === "list")
      return {
        kind: "page",
        cleanup: { running: false, error: null },
        entries: [current],
        next: null,
        supported: true,
      };
    const changed = next(call);
    if (changed) current = changed;
    return { kind: "accepted" };
  });
}

test("a file held for its type asks in the card, with the choice under the question", async () => {
  serving({ ...entry, revision: "00000002", state: "held", received: "100" }, (call) =>
    call.kind === "keep" ? { ...entry, revision: "00000003", state: "completed" } : null,
  );
  await render(DownloadStatus, { props: { profile, onopen: () => {} } });

  await expect.element(page.getByText("setup.exe", { exact: true })).toBeVisible();
  await expect
    .element(page.getByText("This type of file can harm your computer. Keep it?"))
    .toBeVisible();
  await expect
    .element(page.getByRole("button", { name: "Cancel", exact: true }))
    .not.toBeInTheDocument();

  await page.getByRole("button", { name: "Keep", exact: true }).click();

  expect(native.call).toHaveBeenCalledWith(profile, { kind: "keep", id: entry.id });
  await expect
    .element(page.getByRole("button", { name: "Keep", exact: true }))
    .not.toBeInTheDocument();
});

test("a download in flight offers Pause beside Cancel", async () => {
  serving(entry, (call) =>
    call.kind === "pause" ? { ...entry, revision: "00000002", state: "paused" } : null,
  );
  await render(DownloadStatus, { props: { profile, onopen: () => {} } });

  await page.getByRole("button", { name: "Pause", exact: true }).click();

  expect(native.call).toHaveBeenCalledWith(profile, { kind: "pause", id: entry.id });
  await expect.element(page.getByText("Paused", { exact: true })).toBeVisible();
  await expect.element(page.getByRole("button", { name: "Cancel", exact: true })).toBeVisible();
});

test("the card sits flush with the column's edges, which the window already insets", async () => {
  serving(entry, () => null);
  const screen = await render(DownloadStatus, { props: { profile, onopen: () => {} } });
  await expect.element(page.getByText("setup.exe", { exact: true })).toBeVisible();
  const card = screen.container.querySelector(".download-card")!.getBoundingClientRect();
  const column = screen.container.getBoundingClientRect();
  expect(card.left).toBe(column.left);
  expect(card.right).toBe(column.right);
});
