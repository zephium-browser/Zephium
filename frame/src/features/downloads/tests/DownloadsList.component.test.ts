import { expect, test, vi } from "vitest";
import { page } from "vitest/browser";
import { render } from "vitest-browser-svelte";
import type { DownloadCall, DownloadError, DownloadView } from "$shared/ipc/bindings";
import DownloadsList from "../components/DownloadsList.svelte";

const native = vi.hoisted(() => ({
  call: vi.fn(),
  listener: null as null | ((event: { payload: { profile: string } }) => void),
}));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({ downloadCall: native.call });
});
vi.mock("$shared/ipc/native-events", () => ({
  events: {
    downloadsChanged: {
      listen: (listener: typeof native.listener) => {
        native.listener = listener;
        return Promise.resolve(() => {
          native.listener = null;
        });
      },
    },
  },
}));

test("native completion replaces cancellation with ID-scoped open and reveal actions", async () => {
  const profile = "00000000000000000000000001";
  let entry: DownloadView = {
    id: "00000000000000000000000002",
    revision: "00000001",
    created_at: "1",
    filename: "fixture.txt",
    source: "https://example.com",
    source_is_context: false,
    state: "receiving",
    received: "10",
    total: "100",
    error: null,
  };
  native.call.mockImplementation(async (_profile: string, call: DownloadCall) => {
    if (call.kind === "list")
      return {
        kind: "page",
        cleanup: { running: false, error: null },
        entries: [entry],
        next: null,
        supported: true,
      };
    if (call.kind === "updates")
      return {
        kind: "updates",
        cleanup: { running: false, error: null },
        entries: [entry],
        removed: [],
      };
    return { kind: "applied" };
  });
  const screen = await render(DownloadsList, { profile });
  await expect.element(page.getByRole("button", { name: "Cancel", exact: true })).toBeVisible();
  await expect.element(page.getByText("10 B of 100 B", { exact: true })).toBeVisible();
  await expect
    .element(page.getByRole("button", { name: "Open fixture.txt", exact: true }))
    .not.toBeInTheDocument();
  entry = { ...entry, revision: "00000002", state: "cancelling" };
  native.listener?.({ payload: { profile } });
  await expect.element(page.getByText("Cancelling… · example.com", { exact: true })).toBeVisible();
  await expect
    .element(page.getByRole("button", { name: "Cancel", exact: true }))
    .not.toBeInTheDocument();
  await expect
    .element(page.getByRole("button", { name: "Remove from list", exact: true }))
    .not.toBeInTheDocument();
  entry = { ...entry, revision: "00000003", state: "completed", received: "100" };
  native.listener?.({ payload: { profile } });
  const open = page.getByRole("button", { name: "Open fixture.txt", exact: true });
  await expect.element(open).toBeVisible();
  await expect
    .element(page.getByRole("button", { name: "Show in folder", exact: true }))
    .toBeInTheDocument();
  await expect
    .element(page.getByRole("button", { name: "Cancel", exact: true }))
    .not.toBeInTheDocument();
  await open.click();
  expect(native.call).toHaveBeenCalledWith(profile, { kind: "open", id: entry.id });
  await screen.unmount();
  expect(native.listener).toBeNull();
});

test("a folder the system refused says so once and offers another folder", async () => {
  const profile = "00000000000000000000000001";
  const entry: DownloadView = {
    id: "00000000000000000000000003",
    revision: "00000004",
    created_at: "1",
    filename: "Terax_0.8.6_aarch64.dmg",
    source: "https://terax.app",
    source_is_context: false,
    state: "failed",
    received: "0",
    total: null,
    error: "permission",
  };
  native.call.mockImplementation(async (_profile: string, call: DownloadCall) => {
    if (call.kind === "list")
      return {
        kind: "page",
        cleanup: { running: false, error: null },
        entries: [entry],
        next: null,
        supported: true,
      };
    if (call.kind === "updates")
      return {
        kind: "updates",
        cleanup: { running: false, error: null },
        entries: [],
        removed: [],
      };
    return { kind: "applied" };
  });
  const screen = await render(DownloadsList, { profile });
  await expect.element(page.getByText("No access to this folder", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "Change Folder", exact: true }).click();
  expect(native.call).toHaveBeenCalledWith(profile, { kind: "choose_directory" });
  await page.getByRole("button", { name: "Clear finished downloads", exact: true }).click();
  expect(native.call).toHaveBeenCalledWith(profile, { kind: "clear" });
  await screen.unmount();
});

test.each<[DownloadError, string]>([
  ["file_busy", "The destination is busy"],
  ["authentication", "The website denied access"],
  ["certificate", "The secure connection could not be verified"],
  ["server", "The website could not provide the file"],
  ["runtime", "The browser process stopped"],
  ["source", "The download source could not be read"],
])(
  "%s failures retain their cause instead of claiming a lost connection",
  async (error, reason) => {
    const entry: DownloadView = {
      id: "00000000000000000000000004",
      revision: "00000001",
      created_at: "1",
      filename: "fixture.txt",
      source: "https://example.com",
      source_is_context: false,
      state: "failed",
      received: "10",
      total: "100",
      error,
    };
    native.call.mockImplementation(async (_profile: string, call: DownloadCall) =>
      call.kind === "list"
        ? {
            kind: "page",
            cleanup: { running: false, error: null },
            entries: [entry],
            next: null,
            supported: true,
          }
        : {
            kind: "updates",
            cleanup: { running: false, error: null },
            entries: [entry],
            removed: [],
          },
    );
    const screen = await render(DownloadsList, { profile: "00000000000000000000000001" });
    await expect.element(page.getByText(reason, { exact: true })).toBeVisible();
    await expect
      .element(page.getByText("The connection was lost", { exact: true }))
      .not.toBeInTheDocument();
    await expect
      .element(page.getByRole("button", { name: "Retry Download", exact: true }))
      .not.toBeInTheDocument();
    await screen.unmount();
  },
);

test("retry resumes a retained interrupted transfer by ID and keeps cancellation available", async () => {
  const profile = "00000000000000000000000001";
  let entry: DownloadView = {
    id: "00000000000000000000000005",
    revision: "00000001",
    created_at: "1",
    filename: "fixture.txt",
    source: "https://example.com",
    source_is_context: false,
    state: "paused",
    received: "10",
    total: "100",
    error: "connection_lost",
  };
  native.call.mockImplementation(async (_profile: string, call: DownloadCall) => {
    if (call.kind === "resume") {
      entry = { ...entry, revision: "00000002", state: "receiving", error: null };
      return { kind: "accepted" };
    }
    if (call.kind === "list")
      return {
        kind: "page",
        cleanup: { running: false, error: null },
        entries: [entry],
        next: null,
        supported: true,
      };
    return {
      kind: "updates",
      cleanup: { running: false, error: null },
      entries: [entry],
      removed: [],
    };
  });
  const screen = await render(DownloadsList, { profile });
  await expect.element(page.getByText("The connection was lost", { exact: true })).toBeVisible();
  await expect.element(page.getByRole("button", { name: "Cancel", exact: true })).toBeVisible();
  await expect
    .element(page.getByRole("button", { name: "Remove from list", exact: true }))
    .not.toBeInTheDocument();
  await page.getByRole("button", { name: "Retry Download", exact: true }).click();
  expect(native.call).toHaveBeenCalledWith(profile, { kind: "resume", id: entry.id });
  await expect.element(page.getByText("10 B of 100 B", { exact: true })).toBeVisible();
  await expect
    .element(page.getByRole("button", { name: "Retry Download", exact: true }))
    .not.toBeInTheDocument();
  await screen.unmount();
});

test("a resumable short response explains retry without presenting a finished corrupt file", async () => {
  const entry: DownloadView = {
    id: "00000000000000000000000006",
    revision: "00000001",
    created_at: "1",
    filename: "fixture.txt",
    source: "https://example.com",
    source_is_context: false,
    state: "paused",
    received: "10",
    total: "100",
    error: "integrity",
  };
  native.call.mockImplementation(async (_profile: string, call: DownloadCall) =>
    call.kind === "list"
      ? {
          kind: "page",
          cleanup: { running: false, error: null },
          entries: [entry],
          next: null,
          supported: true,
        }
      : {
          kind: "updates",
          cleanup: { running: false, error: null },
          entries: [entry],
          removed: [],
        },
  );
  const screen = await render(DownloadsList, { profile: "00000000000000000000000001" });
  await expect.element(page.getByText("The response ended early", { exact: true })).toBeVisible();
  await expect
    .element(page.getByText("The file was incomplete", { exact: true }))
    .not.toBeInTheDocument();
  await expect
    .element(page.getByRole("button", { name: "Retry Download", exact: true }))
    .toBeVisible();
  await expect
    .element(page.getByText("The response ended early", { exact: true }))
    .toHaveAttribute(
      "title",
      "Retry Download can resume this interrupted transfer for up to five minutes while its tab stays open.",
    );
  await screen.unmount();
});
