import { expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import AudioControl from "../AudioControl.svelte";

test("a playing tab offers mute and asks for the opposite of what it shows", async () => {
  const onToggle = vi.fn();
  const screen = await render(AudioControl, {
    audio: { muted: false, can_mute: true },
    size: 16,
    onToggle,
  });
  const button = screen.getByRole("button", { name: "Mute tab" });
  await expect.element(button).toHaveAttribute("title", "Playing audio");
  await expect.element(button).toHaveAttribute("aria-pressed", "false");
  await button.click();
  expect(onToggle).toHaveBeenCalledWith(true);
});

test("a muted tab says so and offers unmute", async () => {
  const onToggle = vi.fn();
  const screen = await render(AudioControl, {
    audio: { muted: true, can_mute: true },
    size: 16,
    onToggle,
  });
  const button = screen.getByRole("button", { name: "Unmute tab" });
  await expect.element(button).toHaveAttribute("title", "Tab muted");
  await expect.element(button).toHaveAttribute("aria-pressed", "true");
  await expect.element(button).toHaveAttribute("data-muted");
  await button.click();
  expect(onToggle).toHaveBeenCalledWith(false);
});

test("the badge rests on the favicon's corner and the speaker shows only under the pointer", async () => {
  const screen = await render(AudioControl, {
    audio: { muted: false, can_mute: true },
    size: 16,
    offset: 21,
    onToggle: () => {},
  });
  const button = screen.getByRole("button", { name: "Mute tab" }).element() as HTMLElement;
  const badge = button.querySelector(".audio-badge")!;
  const glyph = button.querySelector(".audio-glyph")!;
  expect(getComputedStyle(button).insetInlineStart).toBe("18px");
  expect(button.getBoundingClientRect().width).toBe(22);
  expect(getComputedStyle(badge).opacity).toBe("1");
  expect(getComputedStyle(glyph).opacity).toBe("0");
  await screen.getByRole("button", { name: "Mute tab" }).hover();
  await expect.poll(() => getComputedStyle(glyph).opacity).toBe("1");
  await expect.poll(() => getComputedStyle(badge).opacity).toBe("0");
});

test("without engine mute the badge only informs and never takes the pointer", async () => {
  const screen = await render(AudioControl, {
    audio: { muted: false, can_mute: false },
    size: 16,
    onToggle: () => {},
  });
  const mark = screen.getByRole("img", { name: "Playing audio" });
  await expect.element(mark).toBeInTheDocument();
  expect(getComputedStyle(mark.element()).pointerEvents).toBe("none");
  expect(screen.container.querySelector("button")).toBeNull();
});
