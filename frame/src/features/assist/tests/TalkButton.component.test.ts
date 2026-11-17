import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import "$styles/global.css";
import { emitVoice } from "$shared/testing/voice-session";
import { voiceSession } from "$domain/voice-session";
import TalkButton from "../components/TalkButton.svelte";

const native = vi.hoisted(() => ({ intent: vi.fn() }));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({ voiceSessionView: async () => null, voiceSessionIntent: native.intent });
});

beforeEach(async () => {
  vi.clearAllMocks();
  native.intent.mockResolvedValue(true);
  await voiceSession.init();
  emitVoice();
});

afterEach(() => voiceSession.dispose());

const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

function pointer(button: HTMLElement, type: "pointerdown" | "pointerup") {
  button.dispatchEvent(
    new PointerEvent(type, { bubbles: true, button: 0, pointerId: 1, isPrimary: true }),
  );
}

test("a tap starts a latched turn, and the next tap sends it", async () => {
  const screen = await render(TalkButton);
  const button = screen.getByRole("button", { name: "Talk to Zephium" }).element() as HTMLElement;
  pointer(button, "pointerdown");
  pointer(button, "pointerup");
  expect(native.intent.mock.calls).toEqual([["start"]]);
  emitVoice({ state: { state: "listening", press: "latched" }, mic: true });
  await expect.element(screen.getByRole("button", { pressed: true })).toBeInTheDocument();
  pointer(button, "pointerdown");
  pointer(button, "pointerup");
  expect(native.intent.mock.calls).toEqual([["start"], ["stop"]]);
});

test("holding past the threshold sends on release", async () => {
  const screen = await render(TalkButton);
  const button = screen.getByRole("button", { name: "Talk to Zephium" }).element() as HTMLElement;
  pointer(button, "pointerdown");
  await wait(360);
  pointer(button, "pointerup");
  expect(native.intent.mock.calls).toEqual([["start"], ["stop"]]);
});

test("Escape cancels a listening turn from the keyboard", async () => {
  const screen = await render(TalkButton);
  emitVoice({ state: { state: "listening", press: "latched" }, mic: true });
  const button = screen.getByRole("button", { name: "Talk to Zephium" }).element() as HTMLElement;
  await expect.element(screen.getByRole("button", { pressed: true })).toBeInTheDocument();
  button.focus();
  button.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  expect(native.intent).toHaveBeenCalledWith("cancel");
});
