import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import "$styles/global.css";
import { emitVoice } from "$shared/testing/voice-session";
import { voiceSession } from "$domain/voice-session";
import VoiceIsland from "../components/VoiceIsland.svelte";

const native = vi.hoisted(() => ({ intent: vi.fn(), decide: vi.fn() }));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    voiceSessionView: async () => null,
    voiceSessionIntent: native.intent,
    voiceSessionDecide: native.decide,
  });
});

beforeEach(async () => {
  vi.clearAllMocks();
  native.intent.mockResolvedValue(true);
  native.decide.mockResolvedValue(true);
  await voiceSession.init();
});

afterEach(() => voiceSession.dispose());

const LISTENING = { state: { state: "listening", press: "latched" }, mic: true } as const;

test("the island stays away while idle or unavailable", async () => {
  const screen = await render(VoiceIsland);
  emitVoice();
  expect(screen.container.querySelector(".voice-island")).toBeNull();
  emitVoice({ ...LISTENING, availability: "private" });
  await new Promise((resolve) => setTimeout(resolve, 50));
  expect(screen.container.querySelector(".voice-island")).toBeNull();
  emitVoice(LISTENING);
  await expect.element(screen.getByRole("region", { name: "Voice" })).toBeInTheDocument();
});

test("a latched turn sends or cancels by intent, and shows nothing Rust did not say", async () => {
  const screen = await render(VoiceIsland);
  emitVoice({ ...LISTENING, interim: "book a table" });
  await expect.element(screen.getByText("a table")).toBeInTheDocument();
  await screen.getByRole("button", { name: "Send", exact: true }).click();
  expect(native.intent).toHaveBeenCalledWith("stop");
  // Admission is not a state: the island still listens until Rust says otherwise.
  expect(screen.container.querySelector(".voice-send")).not.toBeNull();
  await screen.getByRole("button", { name: "Stop listening" }).click();
  expect(native.intent).toHaveBeenLastCalledWith("cancel");
});

test("a stale view never replaces a newer one", async () => {
  const screen = await render(VoiceIsland);
  emitVoice({ ...LISTENING, seq: 9_000 });
  await expect
    .element(screen.getByRole("button", { name: "Send", exact: true }))
    .toBeInTheDocument();
  emitVoice({ state: { state: "working", work: "thinking" }, seq: 10 });
  await new Promise((resolve) => setTimeout(resolve, 50));
  expect(screen.container.querySelector(".voice-send")).not.toBeNull();
});

test("a step is decided once, and refusal lets the person answer again", async () => {
  const screen = await render(VoiceIsland);
  emitVoice({
    state: { state: "working", work: "confirming" },
    confirm: { block: 0, index: 0, category: "purchase", label: 'Click "Pay €20"' },
  });
  await expect.element(screen.getByText('Click "Pay €20"')).toBeInTheDocument();
  native.decide.mockResolvedValueOnce(false);
  await screen.getByRole("button", { name: "Approve" }).click();
  expect(native.decide).toHaveBeenCalledWith("approve");
  await expect.element(screen.getByRole("button", { name: "Approve" })).toBeEnabled();
  await screen.getByRole("button", { name: "Decline" }).click();
  expect(native.decide).toHaveBeenLastCalledWith("decline");
  await expect.element(screen.getByRole("button", { name: "Approve" })).toBeDisabled();
  await expect.element(screen.getByRole("button", { name: "Decline" })).toBeDisabled();
});

test("work stops by intent, and faults name their way out", async () => {
  const onaccount = vi.fn();
  const screen = await render(VoiceIsland, { onaccount });
  emitVoice({ state: { state: "working", work: "acting" }, step: 'Click "Next"' });
  await screen.getByRole("button", { name: "Stop the reply" }).click();
  expect(native.intent).toHaveBeenCalledWith("cancel");
  emitVoice({ fault: "out_of_credit" });
  await screen.getByRole("button", { name: "Buy credits" }).click();
  expect(onaccount).toHaveBeenCalledWith("credits");
  emitVoice({ fault: "plan_required" });
  await screen.getByRole("button", { name: "See plans" }).click();
  expect(onaccount).toHaveBeenLastCalledWith("account");
  await screen.getByRole("button", { name: "Dismiss" }).click();
  expect(native.intent).toHaveBeenLastCalledWith("cancel");
});

test("the rail disc says its state to assistive technology", async () => {
  const screen = await render(VoiceIsland, { compact: true });
  emitVoice({ fault: "offline" });
  await expect
    .poll(() => screen.container.querySelector(".voice-disc")?.getAttribute("aria-label"))
    .toMatch(/^Voice: \S/u);
});
