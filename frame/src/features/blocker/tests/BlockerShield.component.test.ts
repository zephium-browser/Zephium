import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import "$styles/global.css";
import "$shared/ui/Menu/surface.css";
import type { BlockerStatusView } from "$shared/ipc/bindings";
import BlockerShieldHost from "./BlockerShieldHost.svelte";

const native = vi.hoisted(() => ({
  refresh: vi.fn(async () => {}),
  setProtected: vi.fn(async (_protect: boolean) => true),
  setEnabled: vi.fn(async (_enabled: boolean) => ({
    state: "processed",
    disposition: { outcome: "applied" },
  })),
  changeSite: vi.fn(async (..._args: unknown[]) => ({
    state: "processed",
    disposition: { outcome: "applied" },
  })),
  hide: vi.fn(async (..._args: unknown[]) => true),
  stats: vi.fn(async () => ({
    status: "ok" as const,
    data: { today: 12345, last7Days: 0, days: [] },
  })),
  status: null as unknown as BlockerStatusView,
}));
vi.mock("$domain/blocker", async (original) => ({
  ...(await original<typeof import("$domain/blocker")>()),
  blocker: { status: () => native.status, refresh: native.refresh, setEnabled: native.setEnabled },
  blockerSites: { setProtected: native.setProtected, changeSite: native.changeSite },
  hiding: { start: native.hide },
}));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({ blockerStats: native.stats });
});

const CONTEXT = { profile: "profile", tab: "tab", site: "meet.example", revision: "1" };

function blocking(
  over: Record<string, unknown> = {},
  site: Record<string, unknown> = {},
): BlockerStatusView {
  return {
    protection: "active",
    applied_enabled: true,
    desired_enabled: true,
    can_enable: true,
    preference: "authoritative",
    site: {
      context: CONTEXT,
      paused: false,
      private_session: false,
      ready: true,
      busy: false,
      hides: [],
      ...site,
    },
    ...over,
  } as unknown as BlockerStatusView;
}

beforeEach(() => {
  native.status = blocking();
});
afterEach(() => vi.clearAllMocks());

const switchOf = (screen: Awaited<ReturnType<typeof render>>) =>
  screen.getByRole("switch", { name: "Block on this site" });

const shield = (props: { open: boolean; headed?: boolean }) => render(BlockerShieldHost, props);

test("closed, it reads nothing and draws no controls, only the row that is the page's", async () => {
  const screen = await shield({ open: false });
  await new Promise((resolve) => setTimeout(resolve, 50));
  expect(native.refresh).not.toHaveBeenCalled();
  await expect.element(screen.getByRole("switch")).not.toBeInTheDocument();
  await expect.element(screen.getByText("Block on this site")).toBeVisible();
});

test("opening reads the standing, and it reads again at every opening", async () => {
  const screen = await shield({ open: true });
  await expect.element(switchOf(screen)).toBeVisible();
  expect(native.refresh).toHaveBeenCalledTimes(1);
  await screen.rerender({ open: false });
  await screen.rerender({ open: true });
  await expect.poll(() => native.refresh.mock.calls.length).toBe(2);
});

test("blocking is one row, the name and the switch, with no count and nothing read for one", async () => {
  const screen = await shield({ open: true });
  await expect.element(switchOf(screen)).toBeChecked();
  expect(screen.container.querySelector(".head .name")!.textContent).toBe("Block on this site");
  expect(screen.container.textContent).not.toMatch(/\d|blocked today/u);
  expect(screen.container.querySelector(".standing")).toBeNull();
  expect(native.stats).not.toHaveBeenCalled();
  await switchOf(screen).click();
  expect(native.setProtected).toHaveBeenCalledExactlyOnceWith(false);
});

test("a paused site is told by its switch, and the switch resumes it", async () => {
  native.status = blocking({}, { paused: true });
  const screen = await shield({ open: true });
  await expect.element(switchOf(screen)).not.toBeChecked();
  expect(screen.container.querySelector(".standing")).toBeNull();
  await switchOf(screen).click();
  expect(native.setProtected).toHaveBeenCalledExactlyOnceWith(true);
});

test("a switch that did not go through says so", async () => {
  native.setProtected.mockResolvedValueOnce(false);
  const screen = await shield({ open: true });
  await switchOf(screen).click();
  await expect.element(screen.getByRole("status")).toHaveTextContent("Couldn't change blocking.");
});

test("blocking that is off has no switch, and offers to turn it on", async () => {
  native.status = blocking({
    protection: "disabled",
    applied_enabled: false,
    desired_enabled: false,
  });
  const screen = await shield({ open: true });
  await expect.element(screen.getByText("Turned off in Settings")).toBeVisible();
  await expect.element(screen.getByRole("switch")).not.toBeInTheDocument();
  expect(screen.container.querySelector(".standing")!.classList.contains("headed")).toBe(true);
  await screen.getByRole("button", { name: "Turn on blocking" }).click();
  expect(native.setEnabled).toHaveBeenCalledExactlyOnceWith(true);
});

test("absent blocking leaves the page's row without a name, and nothing else", async () => {
  native.status = blocking({ protection: "unavailable", applied_enabled: null });
  const screen = await shield({ open: true });
  await expect.element(screen.getByText("Block on this site")).not.toBeInTheDocument();
  await expect.element(screen.getByRole("switch")).not.toBeInTheDocument();
  expect(screen.container.querySelector(".head")).not.toBeNull();
  expect(screen.container.querySelectorAll("button")).toHaveLength(0);
});

test("a page with no site of its own, a new tab, has no section while blocking is in order", async () => {
  native.status = blocking({ site: null });
  const screen = await shield({ open: true, headed: false });
  await expect.poll(() => native.refresh.mock.calls.length).toBe(1);
  expect(screen.container.querySelector("section")).toBeNull();
  expect(native.stats).not.toHaveBeenCalled();
});

test("without a row of its own, blocking that is off still says so and offers to turn on", async () => {
  native.status = blocking({
    protection: "disabled",
    applied_enabled: false,
    desired_enabled: false,
    site: null,
  });
  const screen = await shield({ open: true, headed: false });
  await expect.element(screen.getByText("Turned off in Settings")).toBeVisible();
  expect(screen.container.querySelector(".standing")!.classList.contains("headed")).toBe(false);
  await expect.element(screen.getByRole("button", { name: "Turn on blocking" })).toBeVisible();
});

test("Hide elements starts the picker for this page and leaves the panel", async () => {
  const screen = await shield({ open: true });
  const hide = screen.getByRole("button", { name: "Hide elements…" });
  await expect.element(hide).toHaveAttribute("data-dismiss");
  expect(hide.element().querySelector(".ui-menu-icon svg")).not.toBeNull();
  expect(hide.element().getBoundingClientRect().height).toBe(34);
  await hide.click();
  expect(native.hide).toHaveBeenCalledExactlyOnceWith(CONTEXT);
});

test("a row the keyboard is on has a ring", async () => {
  native.status = blocking({}, { hides: [{ id: "a", label: "Cookie banner" }] });
  const screen = await shield({ open: true });
  await expect.element(switchOf(screen)).toBeVisible();
  const rows = [...screen.container.querySelectorAll<HTMLElement>("button.row")];
  expect(rows).toHaveLength(2);
  for (const row of rows) expect(row.querySelector(".ui-menu-icon svg")).not.toBeNull();

  const hide = screen.getByRole("button", { name: "Hide elements…" });
  hide.element().focus({ focusVisible: true } as FocusOptions);
  await expect.element(hide).toHaveFocus();
  expect(hide.element().matches(":focus-visible")).toBe(true);
  expect(getComputedStyle(hide.element()).outlineStyle).toBe("solid");
});

test("what was hidden is counted, listed on request, and shown again one by one or all", async () => {
  native.status = blocking(
    {},
    {
      hides: [
        { id: "a", label: "Cookie banner" },
        { id: "b", label: "Newsletter" },
      ],
    },
  );
  const screen = await shield({ open: true });
  const hidden = screen.getByRole("button", { name: /Hidden on this site/u });
  await expect.element(hidden).toHaveTextContent("2");
  await expect.element(screen.getByText("Cookie banner")).not.toBeInTheDocument();

  await hidden.click();
  await expect.element(screen.getByText("Cookie banner")).toBeVisible();
  await screen.getByRole("button", { name: "Show Cookie banner again" }).click();
  expect(native.changeSite).toHaveBeenLastCalledWith(CONTEXT, { kind: "remove_hide", id: "a" });

  await screen.getByRole("button", { name: "Show all" }).click();
  expect(native.changeSite).toHaveBeenCalledWith(CONTEXT, { kind: "remove_hide", id: "a" });
  expect(native.changeSite).toHaveBeenLastCalledWith(CONTEXT, { kind: "remove_hide", id: "b" });
});

test("a page whose controls are not ready offers to retry them", async () => {
  native.status = blocking({}, { ready: false });
  const screen = await shield({ open: true });
  await expect.element(screen.getByRole("switch")).not.toBeInTheDocument();
  await screen.getByRole("button", { name: "Retry site controls" }).click();
  expect(native.changeSite).toHaveBeenCalledExactlyOnceWith(CONTEXT, { kind: "retry" });
});
