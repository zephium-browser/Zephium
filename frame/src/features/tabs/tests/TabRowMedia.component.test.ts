import { beforeEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { tabFixture } from "$shared/testing/fixtures";
import favicon from "../../../styles/favicon.css?raw";
import TabRow from "../components/TabRow.svelte";

const style = document.createElement("style");
style.textContent = favicon;
document.head.append(style);

const native = vi.hoisted(() => ({ mute: vi.fn(), menu: vi.fn(), settle: vi.fn() }));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({ tabSetMuted: native.mute, tabMenuPopup: native.menu });
});
vi.mock("$domain/operations", async (original) => ({
  ...(await original<typeof import("$domain/operations")>()),
  settle: native.settle,
}));

beforeEach(() => {
  vi.clearAllMocks();
  native.mute.mockResolvedValue({ accepted: true, operation_id: "mute" });
  native.menu.mockResolvedValue(true);
  native.settle.mockResolvedValue({ outcome: "applied", disposition: null });
});

function row(tab: ReturnType<typeof tabFixture>, onSelect = vi.fn()) {
  return render(TabRow, {
    tab,
    active: false,
    splitCandidate: false,
    onSelect,
    onClose: vi.fn(),
    onContextMenu: vi.fn(),
    onPointerDown: vi.fn(),
    onPointerMove: vi.fn(),
    onPointerUp: vi.fn(),
    onPointerCancel: vi.fn(),
  });
}

test("a quiet tab shows no speaker", async () => {
  const screen = await row(tabFixture());
  expect(screen.container.querySelector("[data-zephium-audio-control]")).toBeNull();
});

test("the speaker mutes its own tab without selecting it", async () => {
  const tab = tabFixture({ audio: { muted: false, can_mute: true } });
  const onSelect = vi.fn();
  const screen = await row(tab, onSelect);
  await screen.getByRole("button", { name: "Mute tab" }).click();
  await expect.poll(() => native.mute.mock.calls).toEqual([[tab.id, true]]);
  expect(onSelect).not.toHaveBeenCalled();
});

test("sound is told on the favicon, so the title keeps its place", async () => {
  const quiet = await row(tabFixture());
  const at = quiet.container.querySelector("[data-zephium-tab-label]")!.getBoundingClientRect();
  quiet.unmount();
  const screen = await row(tabFixture({ audio: { muted: false, can_mute: true } }));
  const label = screen.container.querySelector("[data-zephium-tab-label]")!.getBoundingClientRect();
  expect(label.left).toBe(at.left);
  expect(label.width).toBe(at.width);
  const favicon = screen.container.querySelector("[data-favicon]")!;
  expect(favicon.hasAttribute("data-notched")).toBe(true);
  const mark = screen.getByRole("button", { name: "Mute tab" }).element().getBoundingClientRect();
  const icon = favicon.getBoundingClientRect();
  expect(Math.abs(mark.left + mark.width / 2 - (icon.left + icon.width / 2))).toBeLessThanOrEqual(
    1,
  );
  expect(Math.abs(mark.top + mark.height / 2 - (icon.top + icon.height / 2))).toBeLessThanOrEqual(
    1,
  );
});

test("hovering the favicon of a playing tab turns it into the speaker", async () => {
  const screen = await row(tabFixture({ audio: { muted: false, can_mute: true } }));
  const mark = screen.container.querySelector("[data-favicon] > *")!;
  expect(getComputedStyle(mark).opacity).toBe("1");
  await screen.getByRole("button", { name: "Mute tab" }).hover();
  await expect.poll(() => getComputedStyle(mark).opacity).toBe("0");
});

test("a muted tab keeps its speaker and unmutes", async () => {
  const tab = tabFixture({ audio: { muted: true, can_mute: true } });
  const screen = await row(tab);
  await screen.getByRole("button", { name: "Unmute tab" }).click();
  await expect.poll(() => native.mute.mock.calls).toEqual([[tab.id, false]]);
});

test("the camera indicator offers its own tab's media menu without selecting it", async () => {
  const tab = tabFixture({
    capture: { navigation_id: "000000000000002a", camera: "active", microphone: "none" },
  });
  const onSelect = vi.fn();
  const screen = await row(tab, onSelect);
  await screen.getByRole("button", { name: `Camera and microphone for ${tab.url}` }).click();
  await expect.poll(() => native.menu.mock.calls.length).toBe(1);
  const [id, x, y, context] = native.menu.mock.calls[0]!;
  expect(id).toBe(tab.id);
  expect(typeof x).toBe("number");
  expect(typeof y).toBe("number");
  expect(context).toMatchObject({ media_only: true });
  expect(onSelect).not.toHaveBeenCalled();
  expect(native.mute).not.toHaveBeenCalled();
});

// Sampled over the length of the transitions: a hover that lets go and takes
// hold again shows as a false among the samples.
async function hoverStaysHeld(button: HTMLElement, duration = 600) {
  const samples: boolean[] = [];
  const end = performance.now() + duration;
  while (performance.now() < end) {
    samples.push(button.matches(":hover"));
    await new Promise((resolve) => requestAnimationFrame(resolve));
  }
  return samples;
}

test("resting on the badge's corner holds the hover instead of flashing between badge and speaker", async () => {
  const screen = await row(tabFixture({ audio: { muted: false, can_mute: true } }));
  const mark = screen.getByRole("button", { name: "Mute tab" });
  const button = mark.element() as HTMLElement;
  const badge = button.querySelector(".audio-badge")!.getBoundingClientRect();
  const box = button.getBoundingClientRect();
  // The badge's own bottom inline-end corner, as far from the favicon as it reaches.
  await mark.hover({
    position: { x: badge.right - box.left - 0.5, y: badge.bottom - box.top - 0.5 },
  });
  const samples = await hoverStaysHeld(button);
  expect(samples.length).toBeGreaterThan(10);
  expect(samples.every(Boolean)).toBe(true);
  expect(getComputedStyle(button.querySelector(".audio-glyph")!).opacity).toBe("1");
});

test("the hit area is one fixed box over the favicon, whatever the mark is showing", async () => {
  const screen = await row(tabFixture({ audio: { muted: false, can_mute: true } }));
  const mark = screen.getByRole("button", { name: "Mute tab" });
  const button = mark.element() as HTMLElement;
  const glyph = button.querySelector(".audio-glyph")!;
  const favicon = screen.container.querySelector("[data-favicon]")!.getBoundingClientRect();
  const resting = button.getBoundingClientRect();
  expect(resting.left).toBeLessThanOrEqual(favicon.left);
  expect(resting.top).toBeLessThanOrEqual(favicon.top);
  expect(resting.right).toBeGreaterThanOrEqual(favicon.right);
  expect(resting.bottom).toBeGreaterThanOrEqual(favicon.bottom);

  // The corners of everything the mark draws, resting and speaking.
  const corners = [...button.querySelectorAll(".audio-badge, .audio-glyph")].flatMap((part) => {
    const rect = part.getBoundingClientRect();
    return [
      [rect.left + 0.5, rect.top + 0.5],
      [rect.right - 0.5, rect.top + 0.5],
      [rect.left + 0.5, rect.bottom - 0.5],
      [rect.right - 0.5, rect.bottom - 0.5],
    ] as const;
  });
  const stray = () =>
    corners.filter(([x, y]) => {
      const hit = document.elementFromPoint(x, y);
      return hit !== button && !button.contains(hit);
    });
  expect(stray()).toEqual([]);

  await mark.hover();
  await expect.poll(() => getComputedStyle(glyph).opacity).toBe("1");
  await expect.poll(() => getComputedStyle(glyph).scale).toBe("1");
  expect(stray()).toEqual([]);
  expect(button.getBoundingClientRect().toJSON()).toEqual(resting.toJSON());
});
