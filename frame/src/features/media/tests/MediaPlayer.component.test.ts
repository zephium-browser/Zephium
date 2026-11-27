import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import "$styles/global.css";
import type { MediaPlayerView } from "$shared/ipc/bindings";
import { emitNativeEvent } from "$shared/testing/native-events";
import { mediaPlayer } from "$domain/media-player";
import MediaPlayer from "../components/MediaPlayer.svelte";

const native = vi.hoisted(() => ({ player: vi.fn(), mute: vi.fn(), settle: vi.fn() }));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({ mediaPlayer: native.player, tabSetMuted: native.mute });
});
vi.mock("$domain/operations", async (original) => ({
  ...(await original<typeof import("$domain/operations")>()),
  settle: native.settle,
}));

function view(overrides: Partial<MediaPlayerView> = {}): MediaPlayerView {
  return {
    tab_id: "tab-1",
    title: "Night Drive",
    artist: "Neon Coast",
    icon: null,
    duration: 200,
    position: 65,
    rate: 1,
    playing: true,
    waiting: false,
    can_pip: true,
    in_pip: false,
    muted: false,
    can_mute: true,
    ...overrides,
  };
}

beforeEach(async () => {
  vi.clearAllMocks();
  native.player.mockResolvedValue({ accepted: true, operation_id: "player" });
  native.mute.mockResolvedValue({ accepted: true, operation_id: "mute" });
  native.settle.mockResolvedValue({ outcome: "applied", disposition: null });
  await mediaPlayer.init();
});

afterEach(() => mediaPlayer.dispose());

test("nothing shows until native names a player, and it goes when native says so", async () => {
  const screen = await render(MediaPlayer);
  expect(screen.container.querySelector("section")).toBeNull();
  emitNativeEvent("mediaPlayerChanged", view());
  await expect.element(screen.getByRole("region", { name: "Now playing" })).toBeInTheDocument();
  await expect.element(screen.getByText("Night Drive")).toBeInTheDocument();
  await expect.element(screen.getByText("Neon Coast")).toBeInTheDocument();
  emitNativeEvent("mediaPlayerChanged", null);
  await expect.poll(() => screen.container.querySelector("section")).toBeNull();
});

test("the compact card plays, pauses and mutes its tab", async () => {
  const screen = await render(MediaPlayer);
  emitNativeEvent("mediaPlayerChanged", view());
  await screen.getByRole("button", { name: "Pause", exact: true }).click();
  expect(native.player).toHaveBeenLastCalledWith("tab-1", { kind: "pause" });
  await screen.getByRole("button", { name: "Mute tab" }).click();
  expect(native.mute).toHaveBeenLastCalledWith("tab-1", true);
  emitNativeEvent("mediaPlayerChanged", view({ playing: false, muted: true }));
  await screen.getByRole("button", { name: "Play", exact: true }).click();
  expect(native.player).toHaveBeenLastCalledWith("tab-1", { kind: "play" });
  const unmute = screen.getByRole("button", { name: "Unmute tab" });
  await expect.element(unmute).toHaveAttribute("aria-pressed", "true");
});

test("the rest opens under the pointer: progress, times, picture in picture and close", async () => {
  const screen = await render(MediaPlayer);
  emitNativeEvent("mediaPlayerChanged", view({ playing: false }));
  const card = screen.getByRole("region", { name: "Now playing" });
  await expect.element(card).toBeInTheDocument();
  const more = card.element().querySelector(".media-more")!;
  expect(more.hasAttribute("inert")).toBe(true);
  await card.hover();
  await expect.poll(() => more.hasAttribute("inert")).toBe(false);
  const slider = screen.getByRole("slider", { name: "Playback position" });
  await expect.element(slider).toHaveAttribute("aria-valuetext", "1:05 of 3:20");
  await expect.element(screen.getByText("-2:15")).toBeInTheDocument();
  await screen.getByRole("button", { name: "Picture in picture" }).click();
  expect(native.player).toHaveBeenLastCalledWith("tab-1", { kind: "picture_in_picture" });
  await screen.getByRole("button", { name: "Close player" }).click();
  expect(native.player).toHaveBeenLastCalledWith("tab-1", { kind: "close" });
});

test("seeking asks native and holds its place until the page answers", async () => {
  const screen = await render(MediaPlayer);
  emitNativeEvent("mediaPlayerChanged", view({ playing: false }));
  await screen.getByRole("region", { name: "Now playing" }).hover();
  const slider = screen.getByRole("slider", { name: "Playback position" });
  const input = slider.element() as HTMLInputElement;
  input.value = "150";
  input.dispatchEvent(new Event("change", { bubbles: true }));
  expect(native.player).toHaveBeenLastCalledWith("tab-1", { kind: "seek", seconds: 150 });
  await expect.element(slider).toHaveAttribute("aria-valuetext", "2:30 of 3:20");
});

test("a live stream shows that it is live instead of a length", async () => {
  const screen = await render(MediaPlayer);
  emitNativeEvent("mediaPlayerChanged", view({ duration: null, can_pip: false }));
  await screen.getByRole("region", { name: "Now playing" }).hover();
  await expect.element(screen.getByText("Live")).toBeInTheDocument();
  expect(screen.container.querySelector("input[type=range]")).toBeNull();
  expect(screen.container.querySelector('[aria-label="Picture in picture"]')).toBeNull();
});

test("the site goes to the tab, and the rail's disc plays or pauses", async () => {
  const screen = await render(MediaPlayer);
  emitNativeEvent("mediaPlayerChanged", view());
  await screen.getByRole("button", { name: "Go to the tab playing Night Drive" }).click();
  expect(native.player).toHaveBeenLastCalledWith("tab-1", { kind: "show" });
  screen.unmount();
  const rail = await render(MediaPlayer, { compact: true });
  await rail.getByRole("button", { name: "Pause: Night Drive" }).click();
  expect(native.player).toHaveBeenLastCalledWith("tab-1", { kind: "pause" });
});

test("without engine mute the player offers none", async () => {
  const screen = await render(MediaPlayer);
  emitNativeEvent("mediaPlayerChanged", view({ can_mute: false }));
  await expect
    .element(screen.getByRole("button", { name: "Pause", exact: true }))
    .toBeInTheDocument();
  expect(screen.container.querySelector('[aria-label="Mute tab"]')).toBeNull();
});

test("the card sits flush with the column's edges, which the window already insets", async () => {
  const screen = await render(MediaPlayer);
  emitNativeEvent("mediaPlayerChanged", view());
  const card = screen.getByRole("region", { name: "Now playing" });
  await expect.element(card).toBeInTheDocument();
  const edges = card.element().getBoundingClientRect();
  const column = screen.container.getBoundingClientRect();
  expect(edges.left).toBe(column.left);
  expect(edges.right).toBe(column.right);
});
