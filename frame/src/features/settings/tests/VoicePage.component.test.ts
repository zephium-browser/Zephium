import "$styles/global.css";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page } from "vitest/browser";
import type { VoiceSessionPreferencesV1 } from "$shared/ipc/bindings";
import { emitVoice } from "$shared/testing/voice-session";
import { keymap } from "$domain/keymap";
import { voiceSession } from "$domain/voice-session";
import VoicePageStage from "./VoicePageStage.svelte";

const native = vi.hoisted(() => ({ settings: vi.fn(), set: vi.fn() }));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  const { TALK_KEY } = await import("$shared/testing/voice-session");
  return mockBindings({
    keymapEntries: async () => [TALK_KEY],
    voiceSessionView: async () => null,
    voiceSessionSettings: native.settings,
    voiceSessionSetSettings: native.set,
  });
});

const PREFERENCES: VoiceSessionPreferencesV1 = {
  spoken: true,
  voice: null,
  language: "automatic",
  earcons: true,
};
const stored = (preferences: VoiceSessionPreferencesV1) => ({
  version: 1,
  preferences,
  voices: ["alloy", "cedar", "marin"],
  default_voice: "marin",
});

beforeEach(async () => {
  vi.clearAllMocks();
  native.settings.mockResolvedValue(stored(PREFERENCES));
  native.set.mockImplementation(async (preferences: VoiceSessionPreferencesV1) =>
    stored(preferences),
  );
  await keymap.init();
  await voiceSession.init();
  emitVoice();
});

afterEach(() => voiceSession.dispose());

const shots = "../../../../../target/voice-ui-shots";
const settle = () => new Promise((done) => setTimeout(done, 400));

test("Settings → Voice, in both themes", async () => {
  await page.viewport(900, 900);
  const screen = await render(VoicePageStage);
  await expect.element(screen.getByText("Default (Marin)")).toBeInTheDocument();
  for (const theme of ["dark", "light"]) {
    document.documentElement.dataset.theme = theme;
    await settle();
    await page.screenshot({ path: `${shots}/settings-voice-${theme}.png` });
  }
  emitVoice({ availability: "signed_out" });
  await expect.element(screen.getByRole("button", { name: "Sign in" })).toBeInTheDocument();
  document.documentElement.dataset.theme = "dark";
});

test("a change sends the whole preference set, and only Rust's answer shows", async () => {
  const screen = await render(VoicePageStage);
  const spoken = screen.getByRole("switch", { name: "Spoken replies" });
  await expect.element(spoken).toBeChecked();
  native.set.mockResolvedValueOnce(null);
  await spoken.click();
  expect(native.set).toHaveBeenCalledWith({ ...PREFERENCES, spoken: false });
  await expect.element(screen.getByRole("alert")).toHaveTextContent("That change wasn’t saved.");
  await expect.element(spoken).toBeChecked();
  await spoken.click();
  await expect.element(spoken).not.toBeChecked();
  await expect.element(screen.getByRole("alert")).not.toBeInTheDocument();
});

test("a private window says voice is not for it", async () => {
  const screen = await render(VoicePageStage);
  emitVoice({ availability: "private" });
  await expect.element(screen.getByText("Private windows never use voice.")).toBeInTheDocument();
});
