import { beforeEach, expect, test, vi } from "vitest";
import { page } from "vitest/browser";
import { render } from "vitest-browser-svelte";
import "$styles/global.css";
import type { GlanceView } from "$domain/layout";
import * as m from "$shared/i18n/messages";
import GlanceOverlay from "../components/GlanceOverlay.svelte";

const glanceAct = vi.fn(async (_action: string) => ({ accepted: true, operation_id: null }));
const copyActiveLink = vi.fn(async () => true);

vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({ glanceAct: (action) => glanceAct(action) });
});

vi.mock("$domain/tabs", () => ({ tabs: { copyActiveLink: () => copyActiveLink() } }));

const view: GlanceView = {
  tab: "child",
  parent: "opener",
  region: { x: 0, y: 0, width: 400, height: 500 },
  page: { x: 40, y: 0, width: 320, height: 500 },
  backdrop: null,
};

const actions = () => glanceAct.mock.calls.map(([action]) => action);

beforeEach(async () => {
  await page.viewport(800, 600);
  glanceAct.mockClear();
  copyActiveLink.mockReset();
  copyActiveLink.mockResolvedValue(true);
});

test("its buttons close, expand and split the glance", async () => {
  render(GlanceOverlay, { view });

  await page.getByRole("button", { name: m.glance_expand() }).click();
  await page.getByRole("button", { name: m.glance_split() }).click();
  await page.getByTitle(m.glance_close()).click();

  expect(actions()).toEqual(["expand", "split", "close"]);
});

test("Escape closes it unless something else already handled the key", async () => {
  render(GlanceOverlay, { view });

  const press = (key: string) => {
    const event = new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true });
    document.body.dispatchEvent(event);
    return event;
  };
  const claim = (event: KeyboardEvent) => event.preventDefault();
  document.body.addEventListener("keydown", claim);
  press("Escape");
  document.body.removeEventListener("keydown", claim);
  expect(actions()).toEqual([]);

  press("Enter");
  expect(actions()).toEqual([]);

  const escape = press("Escape");
  expect(actions()).toEqual(["close"]);
  expect(escape.defaultPrevented).toBe(true);
});

test("copying shows a tick that resets after 1.4 seconds, and nothing on failure", async () => {
  render(GlanceOverlay, { view });

  await page.getByRole("button", { name: m.glance_copy() }).click();
  await expect.element(page.getByRole("button", { name: m.glance_copied() })).toBeVisible();
  const shown = performance.now();

  await new Promise((resolve) => setTimeout(resolve, 1000));
  expect(page.getByRole("button", { name: m.glance_copied() }).query()).not.toBeNull();
  await expect
    .element(page.getByRole("button", { name: m.glance_copy() }), { timeout: 2000 })
    .toBeVisible();
  expect(performance.now() - shown).toBeGreaterThanOrEqual(1350);

  copyActiveLink.mockResolvedValueOnce(false);
  await page.getByRole("button", { name: m.glance_copy() }).click();
  await expect.poll(() => copyActiveLink.mock.calls.length).toBe(2);
  await expect.element(page.getByRole("button", { name: m.glance_copy() })).toBeVisible();
  expect(actions()).toEqual([]);
});
