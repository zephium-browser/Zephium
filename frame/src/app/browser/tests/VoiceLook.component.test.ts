import "$styles/global.css";
import { afterEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page } from "vitest/browser";
import type { MediaPlayerView, VoiceSessionViewV1 } from "$shared/ipc/bindings";
import { emitNativeEvent } from "$shared/testing/native-events";
import { emitVoice } from "$shared/testing/voice-session";
import { favicons } from "$domain/favicons";
import { keymap } from "$domain/keymap";
import { mediaPlayer } from "$domain/media-player";
import { preferences } from "$domain/preferences";
import { surface } from "$domain/surface";
import { tabs } from "$domain/tabs";
import { voiceSession } from "$domain/voice-session";
import * as sidebar from "$session/sidebar-mode.svelte";
import Shell from "../Shell.svelte";
import { paint, wallpaper } from "./look-bench";

vi.mock("../WorkWorkspace.svelte", async () => await import("./StageStub.svelte"));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  const { TALK_KEY } = await import("$shared/testing/voice-session");
  return mockBindings({
    pagePermissionCardHost: async () => {},
    tabsBootstrap: async () => {},
    sidebarSetHidden: async () => {},
    sidebarPeek: async () => {},
    sidebarSetWidth: async () => {},
    settingSet: async () => ({ accepted: true, operation_id: null }),
    sidebarResize: async () => false,
    sidebarResizeGuide: async () => true,
    settingGet: async () => null,
    keymapEntries: async () => [TALK_KEY],
    mediaPlayer: async () => ({ accepted: true, operation_id: "player" }),
    voiceSessionView: async () => null,
    voiceSessionIntent: async () => true,
    voiceSessionDecide: async () => true,
  });
});

afterEach(() => {
  sidebar.setMode("default");
  voiceSession.dispose();
  mediaPlayer.dispose();
  favicons.dispose();
  surface.dispose();
  tabs.dispose();
  preferences.dispose();
});

const DIR = "../../../../../target/voice-ui-shots";
const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

const LISTENING: Partial<VoiceSessionViewV1> = {
  state: { state: "listening", press: "held" },
  mic: true,
  interim: "find me a quiet table for two tonight somewhere near the river and book the",
};

/** Every state the island draws, in the order a turn passes through them. */
const STATES: [string, Partial<VoiceSessionViewV1>, [number, number]?][] = [
  ["arming", { state: { state: "arming", press: "held" } }],
  ["listening-held-empty", { state: { state: "listening", press: "held" }, mic: true }, [8, 0]],
  ["listening-held", LISTENING, [64, 0]],
  ["listening-latched", { ...LISTENING, state: { state: "listening", press: "latched" } }, [38, 0]],
  [
    "finalizing",
    {
      state: { state: "finalizing" },
      mic: true,
      warm: true,
      heard: "Find me a quiet table for two tonight near the river and book the earliest one.",
    },
  ],
  [
    "thinking",
    {
      state: { state: "working", work: "thinking" },
      mic: true,
      warm: true,
      heard: "Find me a quiet table for two tonight near the river.",
      phase: { phase: "thinking" },
    },
  ],
  [
    "reading",
    {
      state: { state: "working", work: "reading" },
      heard: "Find me a quiet table for two tonight near the river.",
      phase: { phase: "reading", site: "opentable.com" },
    },
  ],
  [
    "acting",
    {
      state: { state: "working", work: "acting" },
      heard: "Find me a quiet table for two tonight near the river.",
      phase: { phase: "acting" },
      step: 'Choose "7:30 PM" for 2 guests',
    },
  ],
  [
    "acting-speaking",
    {
      state: { state: "working", work: "acting" },
      heard: "Find me a quiet table for two tonight near the river.",
      phase: { phase: "acting" },
      step: 'Choose "7:30 PM" for 2 guests',
      speaking: true,
    },
    [0, 52],
  ],
  [
    "confirming",
    {
      state: { state: "working", work: "confirming" },
      confirm: { block: 1, index: 2, category: "purchase", label: 'Click "Reserve · €20 deposit"' },
    },
  ],
  [
    "confirming-mic",
    {
      state: { state: "working", work: "confirming" },
      mic: true,
      warm: true,
      confirm: { block: 1, index: 0, category: "communication", label: 'Click "Send"' },
    },
  ],
  [
    "speaking",
    { state: { state: "settled", outcome: "acted" }, outcome: "acted", speaking: true },
    [0, 70],
  ],
  ["speaking-answer", { state: { state: "idle" }, speaking: true }, [0, 40]],
  ...(
    [
      "answered",
      "acted",
      "partial",
      "not_confirmed",
      "failed",
      "declined",
      "cancelled",
      "needs_you",
    ] as const
  ).map((outcome): [string, Partial<VoiceSessionViewV1>] => [
    `settled-${outcome}`,
    { state: { state: "settled", outcome }, outcome },
  ]),
  ...(
    [
      "microphone_denied",
      "no_microphone",
      "offline",
      "plan_required",
      "out_of_credit",
      "daily_limit",
      "rate_limited",
      "provider_down",
      "timeout",
      "interrupted",
      "busy",
      "unavailable",
    ] as const
  ).map((fault): [string, Partial<VoiceSessionViewV1>] => [`fault-${fault}`, { fault }]),
  ["warm", { mic: true, warm: true }],
];

function player(over: Partial<MediaPlayerView> = {}): MediaPlayerView {
  return {
    tab_id: "spotify",
    title: "Night Drive",
    artist: "Neon Coast",
    icon: { origin: "https://spotify.example", revision: "a" },
    duration: 200,
    position: 50,
    rate: 1,
    playing: false,
    waiting: false,
    can_pip: false,
    in_pip: false,
    muted: false,
    can_mute: true,
    ...over,
  };
}

async function stage(theme: "light" | "dark", compact: boolean) {
  document.documentElement.dataset.theme = theme;
  wallpaper("mono", theme, theme === "dark" ? "#1b1b1e" : "#fbfbfb", {
    inset: compact ? "8px 8px 8px 72px" : "8px 8px 8px 256px",
  });
  await paint();
  await keymap.init();
  await mediaPlayer.init();
  await voiceSession.init();
  const screen = await render(Shell);
  if (compact) sidebar.setMode("compact");
  await wait(300);
  return screen;
}

const column = () => document.querySelector<HTMLElement>(".browser-sidebar")!;
const snap = (name: string) =>
  page.elementLocator(column()).screenshot({ path: `${DIR}/${name}.png` });

for (const theme of ["dark", "light"] as const)
  test(`the island in the column, ${theme}`, async () => {
    await page.viewport(900, 640);
    await stage(theme, false);
    for (const [name, over, levels] of STATES) {
      emitVoice(over, levels);
      await expect.poll(() => document.querySelector(".voice-island")).not.toBeNull();
      await wait(260);
      await snap(`column-${theme}-${name}`);
    }
    // With something playing, voice stands above the player, which steps back.
    emitNativeEvent("mediaPlayerChanged", player());
    emitVoice(LISTENING, [55, 0]);
    await expect.poll(() => document.querySelector(".media-player")).not.toBeNull();
    await wait(300);
    await snap(`column-${theme}-with-media`);
    emitVoice({ state: { state: "idle" } });
    await expect.poll(() => document.querySelector(".voice-island")).toBeNull();
    await wait(300);
    await snap(`column-${theme}-media-alone`);
  });

for (const theme of ["dark", "light"] as const)
  test(`the island in the rail, ${theme}`, async () => {
    await page.viewport(900, 640);
    await stage(theme, true);
    for (const [name, over, levels] of STATES.filter(([name]) =>
      [
        "listening-held",
        "listening-latched",
        "thinking",
        "acting",
        "speaking",
        "confirming",
        "settled-acted",
        "settled-not_confirmed",
        "fault-microphone_denied",
      ].includes(name),
    )) {
      emitVoice(over, levels);
      await expect.poll(() => document.querySelector(".voice-disc")).not.toBeNull();
      await wait(260);
      await snap(`rail-${theme}-${name}`);
    }
    emitNativeEvent("mediaPlayerChanged", player());
    emitVoice(LISTENING, [70, 0]);
    await wait(300);
    await snap(`rail-${theme}-with-media`);
    // A step waiting on the person opens the column for its card.
    emitVoice(STATES.find(([name]) => name === "confirming")![1]);
    await expect.poll(() => document.querySelector(".voice-disc")).not.toBeNull();
    document.querySelector<HTMLElement>(".voice-disc")!.click();
    await expect.poll(() => document.querySelector(".voice-confirm")).not.toBeNull();
    await wait(400);
    await snap(`rail-${theme}-confirm-opened`);
    // Answered, the column goes back to the rail it was.
    emitVoice({ state: { state: "settled", outcome: "acted" }, outcome: "acted" });
    await expect.poll(() => document.querySelector(".voice-disc")).not.toBeNull();
  });

test("reduced motion holds the meter still and keeps the live colour", async () => {
  document.documentElement.dataset.reduceMotion = "true";
  try {
    await page.viewport(900, 640);
    await stage("dark", false);
    emitVoice(LISTENING, [90, 0]);
    await expect.poll(() => document.querySelector(".voice-island")).not.toBeNull();
    await wait(200);
    await snap("column-dark-reduced-listening");
    emitVoice({ ...LISTENING }, [5, 0]);
    await wait(120);
    const heights = () =>
      [...document.querySelectorAll<HTMLElement>(".voice-island .bar")].map((bar) =>
        bar.style.getPropertyValue("--level"),
      );
    const before = heights();
    emitVoice({ ...LISTENING }, [95, 0]);
    await wait(120);
    expect(heights()).toEqual(before);
    emitVoice({
      state: { state: "working", work: "thinking" },
      heard: "Find me a quiet table for two tonight.",
    });
    await wait(200);
    await snap("column-dark-reduced-thinking");
  } finally {
    delete document.documentElement.dataset.reduceMotion;
  }
});
