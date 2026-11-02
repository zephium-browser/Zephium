import "$styles/global.css";
import { expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page } from "vitest/browser";
import type { VoiceChanged, VoiceStartV1 } from "$shared/ipc/bindings";
import { uiCommands } from "$domain/ui-commands";
import { emitNativeEvent } from "$shared/testing/native-events";
import VoiceStage from "./VoiceStage.svelte";

const native = vi.hoisted(() => ({ start: vi.fn<() => Promise<VoiceStartV1>>() }));

vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    voiceStart: native.start,
    voiceStop: vi.fn(async () => true),
    voiceCancel: vi.fn(async () => true),
  });
});

const shots = "../../../../../target/work-voice";
const settle = () => new Promise((done) => setTimeout(done, 450));

/** A voice speaking in phrases, fed as native sends it: three levels a tenth of a second. */
function speak(take: number) {
  let step = 0;
  const timer = setInterval(() => {
    const levels = [0, 1, 2].map((at) => {
      const t = step * 3 + at;
      const phrase = Math.sin(t / 7) > -0.2 ? 1 : 0.08;
      return Math.round(phrase * (45 + 40 * Math.abs(Math.sin(t * 1.7))));
    });
    step += 1;
    emitNativeEvent("voiceLevels", { take, levels });
  }, 100);
  return () => clearInterval(timer);
}

const change = (take: number, rest: Partial<VoiceChanged>): VoiceChanged => ({
  take,
  phase: "listening",
  engine: "on_device",
  partial: null,
  text: null,
  fault: null,
  ...rest,
});

function screenshotTarget(root: HTMLElement) {
  // The clock and partial transcript change while a screenshot is resolving.
  root.setAttribute("data-testid", "voice-look-bar");
  return page.getByTestId("voice-look-bar");
}

async function shoot(root: HTMLElement, name: string) {
  for (const theme of ["dark", "light"]) {
    document.documentElement.dataset.theme = theme;
    await settle();
    await screenshotTarget(root).screenshot({ path: `${shots}/${name}-${theme}.png` });
  }
  document.documentElement.dataset.theme = "dark";
}

test("a screenshot target keeps the bar after the live recording clock changes", async () => {
  await page.viewport(1000, 420);
  const screen = await render(VoiceStage);
  try {
    native.start.mockResolvedValue({ take: 13, engine: "zephium", fault: null });
    await page.getByRole("button", { name: "Dictate" }).click();
    await expect.element(page.getByRole("button", { name: "Stop and insert" })).toBeVisible();
    const bar = screen.container.querySelector<HTMLElement>(".work-bar")!;
    const clock = bar.querySelector<HTMLElement>(".time")!;
    const before = clock.textContent;
    const target = screenshotTarget(bar);
    // A one-second label can need the following 250 ms sample to advance.
    await expect.poll(() => clock.textContent, { timeout: 2_000 }).not.toBe(before);
    expect(target.query()).toBe(bar);
  } finally {
    await screen.unmount();
  }
});

test("the bar at rest, listening, transcribing and when it cannot listen, in both themes", async () => {
  await page.viewport(1000, 420);
  const screen = await render(VoiceStage);
  let quiet = () => {};
  try {
    const bar = () => screen.container.querySelector<HTMLElement>(".work-bar")!;
    await shoot(bar(), "rest");

    native.start.mockResolvedValue({ take: 1, engine: "on_device", fault: null });
    await page.getByRole("button", { name: "Dictate" }).click();
    quiet = speak(1);
    await new Promise((done) => setTimeout(done, 2400));
    await shoot(bar(), "listening");
    emitNativeEvent(
      "voiceChanged",
      change(1, { partial: "Find me a quiet hotel in Lisbon for next weekend" }),
    );
    await shoot(bar(), "listening-words");
    quiet();
    emitNativeEvent("voiceChanged", change(1, { phase: "done", text: "Find me a quiet hotel." }));
    await settle();

    native.start.mockResolvedValue({ take: 2, engine: "zephium", fault: null });
    // With words in the field the mic has given way to Send: a take starts from the shortcut.
    await uiCommands.init();
    emitNativeEvent("uiCommand", "work.dictate");
    quiet = speak(2);
    await new Promise((done) => setTimeout(done, 1600));
    await shoot(bar(), "listening-cloud");
    quiet();
    emitNativeEvent("voiceChanged", change(2, { phase: "transcribing", engine: "zephium" }));
    await shoot(bar(), "transcribing");
    emitNativeEvent("voiceChanged", change(2, { phase: "failed", fault: "no_engine" }));
    await shoot(bar(), "no-engine");
  } finally {
    document.documentElement.dataset.theme = "dark";
    quiet();
    uiCommands.dispose();
    await screen.unmount();
  }
});
