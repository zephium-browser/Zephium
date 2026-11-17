import { expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import RailSpaces from "../components/RailSpaces.svelte";
import type { SpaceView } from "$shared/ipc/bindings";

const fixture = vi.hoisted(() => ({
  active: { id: "space", name: "Main", icon: null, color: 0, theme: null, tabs: 0, previews: [] },
}));
const active: SpaceView = fixture.active;
vi.mock("$domain/tabs", () => ({ tabs: { spaces: () => [fixture.active] } }));
vi.mock("../lib/space-requests.svelte", () => ({ openInColumn: () => {} }));

test("unmounting an open space stack removes its modifier listeners", async () => {
  const added = vi.spyOn(window, "addEventListener");
  const removed = vi.spyOn(window, "removeEventListener");
  const screen = await render(RailSpaces, { active });
  const before = added.mock.calls.length;
  await screen.getByRole("button", { name: "Spaces — Main" }).click();
  const subscriptions = added.mock.calls
    .slice(before)
    .filter(([type]) => type === "keydown" || type === "keyup" || type === "blur");
  expect(subscriptions.length).toBeGreaterThanOrEqual(3);
  await screen.unmount();
  for (const [type, listener] of subscriptions)
    expect(
      removed.mock.calls.some(([name, callback]) => name === type && callback === listener),
    ).toBe(true);
});
