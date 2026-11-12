import "$styles/global.css";
import { expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page } from "vitest/browser";
import { WorkEnvironmentSession } from "$domain/work-environment";
import type { WorkEnvironmentSnapshot } from "$shared/ipc/bindings";
import { tabFixture } from "$shared/testing/fixtures";
import WorkEnvironmentWorkspace from "../components/WorkEnvironmentWorkspace.svelte";
const native = vi.hoisted(() => ({
  call: vi.fn(),
  paneShow: vi.fn(),
  paneHide: vi.fn(),
  paneRect: vi.fn(),
  settle: vi.fn(),
}));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockComposerBindings } = await import("./composer-fixtures");
  return mockComposerBindings({
    workCall: native.call,
    workPaneShow: native.paneShow,
    workPaneHide: native.paneHide,
    workPaneSetRect: native.paneRect,
  });
});
vi.mock("$domain/operations", () => ({ settle: native.settle }));

const snapshot: WorkEnvironmentSnapshot = {
  version: 1,
  id: "00000000000000000000000003",
  profile: "00000000000000000000000001",
  space: "00000000000000000000000002",
  title: "Manual research",
  lifecycle: "active",
  revision: "1",
  elements: [],
  areas: [],
  view: { revision: "1", x: 0, y: 0, zoom_milli: 1000, placements: [] },
};

test.each(["applied", "no_op", "deferred", "rejected", "failed"] as const)(
  "a pane request settles %s without inventing a page",
  async (outcome) => {
    vi.clearAllMocks();
    await page.viewport(1200, 800);
    native.call.mockResolvedValue({
      version: 1,
      profile: snapshot.profile,
      reply: { kind: "environment", reply: { kind: "snapshot", snapshot } },
    });
    native.paneShow.mockResolvedValue({ accepted: true, operation_id: "0000000000000001" });
    native.settle.mockResolvedValue({ outcome, disposition: null });
    const session = new WorkEnvironmentSession(snapshot.profile, snapshot.space);
    session.snapshot = snapshot;
    session.selected = snapshot.id;
    const screen = await render(WorkEnvironmentWorkspace, {
      session,
      tabs: [tabFixture({ id: "space-tab", title: "Research tab" })],
      spaceName: "Personal",
      aiEnabled: false,
      onopen: vi.fn(),
      onnewtab: vi.fn(),
    });
    const root = screen.container.querySelector(".environment") as HTMLElement;
    root.style.height = "720px";
    root.style.width = "1100px";
    await screen.getByRole("button", { name: "Add to canvas", exact: true }).click();
    await screen.getByRole("button", { name: "Open Research tab here", exact: true }).click();
    await expect.poll(() => native.settle.mock.calls.length).toBe(1);
    expect(native.paneShow.mock.lastCall?.[0]).toEqual({ kind: "tab", id: "space-tab" });
    const pane = screen.getByRole("region", { name: "Browser pane", exact: true });
    if (outcome === "rejected" || outcome === "failed") {
      await expect.element(screen.getByText("This page could not be opened here.")).toBeVisible();
      await expect.element(pane).not.toBeInTheDocument();
      expect(native.paneHide).not.toHaveBeenCalled();
    } else {
      await expect.element(pane).toBeVisible();
      expect(screen.container.textContent).not.toContain("could not be opened");
      await expect.element(screen.getByText("Opening…")).toBeVisible();
    }
    await screen.unmount();
    session.dispose();
  },
);
