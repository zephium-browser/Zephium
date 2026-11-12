import "$styles/global.css";
import { afterEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page, userEvent } from "vitest/browser";
import type { VoiceChanged, VoiceStartV1 } from "$shared/ipc/bindings";
import { uiCommands } from "$domain/ui-commands";
import { emitNativeEvent } from "$shared/testing/native-events";
import VoiceStage from "./VoiceStage.svelte";

const native = vi.hoisted(() => ({
  start: vi.fn<(request: string) => Promise<VoiceStartV1>>(),
  stop: vi.fn(async (_take: number) => true),
  cancel: vi.fn(async () => true),
  run: vi.fn(async (_id: string) => ({ kind: "accepted" })),
  privacy: vi.fn(async (_kind: string) => true),
}));

vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    voiceStart: native.start,
    voiceStop: native.stop,
    voiceCancel: native.cancel,
    runCommand: native.run as never,
    voiceOpenPrivacy: native.privacy as never,
  });
});

afterEach(() => vi.clearAllMocks());

const request = () => native.start.mock.calls.at(-1)?.[0] ?? "";

const change = (take: number, rest: Partial<VoiceChanged>): VoiceChanged => ({
  take,
  request: request(),
  phase: "listening",
  engine: "on_device",
  partial: null,
  text: null,
  fault: null,
  ...rest,
});

/** With words in the field the mic gives way to Send, so a take starts from the shortcut. */
async function listening(engine: VoiceStartV1["engine"] = "on_device", take = 7, shortcut = false) {
  native.start.mockResolvedValue({ take, engine, fault: null });
  if (shortcut) {
    await uiCommands.init();
    emitNativeEvent("uiCommand", "work.dictate");
  } else await page.getByRole("button", { name: "Dictate" }).click();
  await expect.element(page.getByRole("button", { name: "Stop and insert" })).toBeVisible();
  await expect.poll(() => native.start.mock.calls.length).toBe(1);
}

const field = () => document.querySelector<HTMLTextAreaElement>(".work-bar textarea")!;

test("the mic opens a live take, and stopping lands the words in the field", async () => {
  await page.viewport(1000, 600);
  const screen = await render(VoiceStage);
  await listening();
  emitNativeEvent("voiceLevels", { take: 7, request: request(), levels: [30, 60, 80] });
  emitNativeEvent("voiceChanged", change(7, { partial: "Book a table" }));
  await expect.element(page.getByText("Book a table", { exact: true })).toBeVisible();
  await expect.element(page.getByRole("button", { name: "Stop and insert" })).toHaveFocus();
  await page.getByRole("button", { name: "Stop and insert" }).click();
  expect(native.stop).toHaveBeenCalledWith(7);
  emitNativeEvent("voiceChanged", change(7, { phase: "done", text: " Book a table for two. " }));
  await expect.poll(() => field().value).toBe("Book a table for two.");
  await expect.element(page.getByRole("button", { name: "Dictate" })).toBeVisible();
  await expect.poll(() => document.activeElement).toBe(field());
  await screen.unmount();
});

test("dictated words join what was already typed, and other takes' changes are ignored", async () => {
  await page.viewport(1000, 600);
  const screen = await render(VoiceStage, { value: "Find flights " });
  await listening("on_device", 7, true);
  emitNativeEvent(
    "voiceChanged",
    change(7, { request: "00000000-0000-0000-0000-000000000001", phase: "done", text: "Ignored" }),
  );
  emitNativeEvent("voiceChanged", change(3, { phase: "done", text: "Ignored" }));
  emitNativeEvent("voiceChanged", change(7, { phase: "done", text: "to Lisbon" }));
  await expect.poll(() => field().value).toBe("Find flights to Lisbon");
  await screen.unmount();
});

test("Escape cancels the take and keeps what was typed", async () => {
  await page.viewport(1000, 600);
  const screen = await render(VoiceStage, { value: "Plan:" });
  await listening("on_device", 7, true);
  await userEvent.keyboard("{Escape}");
  await expect.poll(() => native.cancel.mock.calls.length).toBe(1);
  await expect.element(page.getByRole("button", { name: "Dictate" })).toBeVisible();
  expect(field().value).toBe("Plan:");
  await screen.unmount();
});

test("the shortcut starts a take and stops it", async () => {
  await page.viewport(1000, 600);
  await uiCommands.init();
  const screen = await render(VoiceStage);
  native.start.mockResolvedValue({ take: 9, engine: "on_device", fault: null });
  emitNativeEvent("uiCommand", "work.dictate");
  await expect.element(page.getByRole("button", { name: "Stop and insert" })).toBeVisible();
  await expect.poll(() => native.start.mock.calls.length).toBe(1);
  emitNativeEvent("uiCommand", "work.dictate");
  await expect.poll(() => native.stop.mock.calls).toEqual([[9]]);
  await screen.unmount();
});

test("a cloud take names where the audio goes, then transcribes", async () => {
  await page.viewport(1000, 600);
  const screen = await render(VoiceStage);
  await listening("open_ai");
  await expect.element(page.getByText("OpenAI", { exact: true })).toBeVisible();
  emitNativeEvent("voiceChanged", change(7, { phase: "transcribing", engine: "open_ai" }));
  await expect.element(page.getByRole("button", { name: "Transcribing…" })).toBeDisabled();
  emitNativeEvent(
    "voiceChanged",
    change(7, { phase: "failed", engine: "open_ai", fault: "key_refused" }),
  );
  await expect
    .element(page.getByRole("alert"))
    .toHaveTextContent("OpenAI refused your key. Check it in Settings → AI.");
  await screen.unmount();
});

test("with no engine the bar says so and opens Settings → AI", async () => {
  await page.viewport(1000, 600);
  const screen = await render(VoiceStage);
  native.start.mockResolvedValue({ take: null, engine: null, fault: "no_engine" });
  await page.getByRole("button", { name: "Dictate" }).click();
  await expect.element(page.getByRole("alert")).toBeVisible();
  await page.getByRole("button", { name: "Settings" }).click();
  expect(native.run).toHaveBeenCalledWith("settings.ai");
  await expect.element(page.getByRole("button", { name: "Dictate" })).toBeVisible();
  await screen.unmount();
});

test("silence ends in a gentle retry, and a refused microphone opens Privacy settings", async () => {
  await page.viewport(1000, 600);
  const screen = await render(VoiceStage);
  await listening();
  emitNativeEvent("voiceChanged", change(7, { phase: "done", text: "" }));
  await expect.element(page.getByRole("alert")).toHaveTextContent("Didn’t catch that.");
  native.start.mockResolvedValue({ take: null, engine: null, fault: "microphone_denied" });
  await page.getByRole("button", { name: "Try again" }).click();
  await expect.poll(() => native.start.mock.calls.length).toBe(2);
  await page.getByRole("button", { name: "Open Privacy Settings" }).click();
  expect(native.privacy).toHaveBeenCalledWith("microphone");
  await screen.unmount();
});

test("a Mac with Dictation off says so and opens Keyboard settings, not a retry", async () => {
  await page.viewport(1000, 600);
  const screen = await render(VoiceStage);
  await listening();
  emitNativeEvent("voiceChanged", change(7, { phase: "failed", fault: "dictation_off" }));
  await expect.element(page.getByRole("alert")).toHaveTextContent("Dictation is off on this Mac.");
  await page.getByRole("button", { name: "Open Keyboard Settings" }).click();
  expect(native.privacy).toHaveBeenCalledWith("dictation");
  await screen.unmount();
});

test("a hosted take refused for its plan opens the account, not a retry", async () => {
  await page.viewport(1000, 600);
  const screen = await render(VoiceStage);
  await listening("zephium");
  emitNativeEvent(
    "voiceChanged",
    change(7, { phase: "failed", engine: "zephium", fault: "plan_required" }),
  );
  await expect
    .element(page.getByRole("alert"))
    .toHaveTextContent("Dictation needs a different Zephium plan.");
  await page.getByRole("button", { name: "See plans" }).click();
  expect(native.run).toHaveBeenCalledWith("settings.account");
  await screen.unmount();
});

test("a recording that runs past four minutes says so and offers another take", async () => {
  await page.viewport(1000, 600);
  const screen = await render(VoiceStage);
  await listening("zephium");
  emitNativeEvent(
    "voiceChanged",
    change(7, { phase: "failed", engine: "zephium", fault: "audio_too_long" }),
  );
  await expect
    .element(page.getByRole("alert"))
    .toHaveTextContent("Recordings can be up to 4 minutes.");
  await expect.element(page.getByRole("button", { name: "Try again" })).toBeVisible();
  await screen.unmount();
});
