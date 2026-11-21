import "$styles/global.css";
import { afterEach, expect, test, vi } from "vitest";
import { page } from "vitest/browser";
import { render } from "vitest-browser-svelte";
import { emitNativeEvent } from "$shared/testing/native-events";
import { tabFixture, revision, spaceFixture } from "$shared/testing/fixtures";
import { accountFixture } from "$shared/testing/account";
import { tabs } from "$domain/tabs";
import { surface } from "$domain/surface";
import Shell from "../Shell.svelte";

vi.mock("$shared/platform", () => ({ IS_MAC: false, IS_WINDOWS: true }));
vi.mock("../WorkWorkspace.svelte", async () => await import("./StageStub.svelte"));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    accountGet: async () => ({
      account: accountFixture(),
      fault: null,
      retry_after: null,
    }),
    pagePermissionCardHost: async () => {},
    tabsBootstrap: async () => {},
  });
});

afterEach(() => {
  surface.dispose();
  tabs.dispose();
});

test("jumping from settings search keeps the sidebar header visible", async () => {
  await page.viewport(1536, 864);
  await surface.init();
  await tabs.init();
  const tab = tabFixture();
  emitNativeEvent("itemsChanged", {
    profile: { id: "profile", name: "Personal", kind: "default" as const },
    spaces: [spaceFixture("space", "Home")],
    active_space_id: "space",
    nodes: [],
    split_group: null,
    projection_revision: revision(10),
    tabs: [tab],
    active: tab.id,
  });

  const screen = await render(Shell);
  emitNativeEvent("uiCommand", "browser.settings");

  const header = await expect
    .poll(() => screen.container.querySelector<HTMLElement>(".settings-panel-header"))
    .not.toBeNull()
    .then(() => screen.container.querySelector<HTMLElement>(".settings-panel-header")!);

  const initialTop = header.getBoundingClientRect().top;
  expect(initialTop).toBeGreaterThanOrEqual(0);

  const search = screen.getByRole("searchbox", { name: /Search settings/u });
  await search.fill("zoom");

  await screen.getByRole("button", { name: /Default zoom/u }).click();

  const highlighted = await expect
    .poll(() =>
      screen.container.querySelector<HTMLElement>(
        '[data-setting="appearance.zoom"][data-highlighted]',
      ),
    )
    .not.toBeNull()
    .then(() =>
      screen.container.querySelector<HTMLElement>(
        '[data-setting="appearance.zoom"][data-highlighted]',
      )!,
    );

  await new Promise((resolve) => requestAnimationFrame(resolve));

  const content = screen.container.querySelector<HTMLElement>(".settings-content")!;
  const contentBounds = content.getBoundingClientRect();
  const targetBounds = highlighted.getBoundingClientRect();
  expect(targetBounds.top).toBeGreaterThanOrEqual(contentBounds.top);
  expect(targetBounds.bottom).toBeLessThanOrEqual(contentBounds.bottom);

  const toolHost = screen.container.querySelector<HTMLElement>(".sidebar-tool-host");
  const nav = screen.container.querySelector<HTMLElement>(".settings-sidebar-navigation");
  expect({
    headerTop: Math.round(header.getBoundingClientRect().top),
    docScrollTop: document.documentElement.scrollTop,
    bodyScrollTop: document.body.scrollTop,
    toolHostScrollTop: toolHost?.scrollTop ?? -1,
    navScrollTop: nav?.scrollTop ?? -1,
  }).toEqual({
    headerTop: Math.round(initialTop),
    docScrollTop: 0,
    bodyScrollTop: 0,
    toolHostScrollTop: 0,
    navScrollTop: 0,
  });
});
