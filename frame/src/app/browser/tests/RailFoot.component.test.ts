import "$styles/global.css";
import { afterEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page } from "vitest/browser";
import { emitNativeEvent } from "$shared/testing/native-events";
import { favicons } from "$domain/favicons";
import { surface } from "$domain/surface";
import { tabs } from "$domain/tabs";
import { preferences } from "$domain/preferences";
import * as sidebar from "$session/sidebar-mode.svelte";
import Shell from "../Shell.svelte";
import { paint, settle } from "./look-bench";

vi.mock("../WorkWorkspace.svelte", async () => await import("./StageStub.svelte"));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  const { accountFixture } = await import("$shared/testing/account");
  const id = "01J00000000000000000000001";
  return mockBindings({
    pagePermissionCardHost: async () => {},
    accountGet: async () => ({ account: accountFixture(), fault: null, retry_after: null }),
    profilesGet: async () => ({
      status: "ok" as const,
      data: {
        current: id,
        private: false,
        profiles: [
          {
            id,
            name: "Ada Lovelace",
            kind: "default" as const,
            avatar: { style: "initials" as const, color: 1, symbol: 0, revision: 1 },
          },
        ],
      },
    }),
    tabsBootstrap: async () => {},
    sidebarSetHidden: async () => {},
    sidebarPeek: async () => {},
    sidebarSetWidth: async () => {},
    settingSet: async () => ({ accepted: true, operation_id: null }),
    sidebarResize: async () => false,
    sidebarResizeGuide: async () => true,
    settingGet: async () => null,
    keymapEntries: async () => [],
  });
});

afterEach(() => {
  sidebar.setMode("default");
  favicons.dispose();
  surface.dispose();
  tabs.dispose();
  preferences.dispose();
});

const box = (element: Element | null) => {
  const rect = element?.getBoundingClientRect();
  return rect && { x: rect.x, y: rect.y, w: rect.width, h: rect.height };
};

/** Where the foot's pieces stand, from the window's bottom edge: Work and Browse must agree. */
function foot(root: ParentNode, trigger: string) {
  const dock = root.querySelector(".dock");
  const case_ = root.querySelector(trigger);
  const rules = [...root.querySelectorAll(".dock-rule")].map(box);
  return {
    dock: box(dock),
    trigger: box(case_),
    rules,
    avatar: box(case_?.querySelector(".profile-avatar, svg") ?? null),
  };
}

test("the rail's foot stands in the same place in Browse and in Work", async () => {
  await page.viewport(900, 640);
  await paint();
  const screen = await render(Shell);
  // The rail a Mac now keeps for Work and the tools, set directly.
  sidebar.setMode("compact");
  await settle();
  const shoot = async (name: string) => {
    for (const theme of ["dark", "light"]) {
      document.documentElement.dataset.theme = theme;
      await settle();
      await page.screenshot({ path: `../../../../../target/work-polish/${name}-${theme}.png` });
    }
    document.documentElement.dataset.theme = "dark";
  };
  await shoot("foot-browse");
  const browse = foot(screen.container, ".shelf .case");
  emitNativeEvent("uiCommand", "browser.work");
  await expect.poll(() => screen.container.querySelector(".work-case")).not.toBeNull();
  await settle();
  await shoot("foot-work");
  const work = foot(screen.container, ".work-case");
  const stand = (side: typeof browse) => {
    const rule = side.rules.at(-1)!;
    return {
      size: [side.trigger!.w, side.trigger!.h],
      x: side.trigger!.x,
      bottom: window.innerHeight - (side.trigger!.y + side.trigger!.h),
      rule: [rule.x, rule.w, side.trigger!.y - (rule.y + rule.h)],
      avatar: [side.avatar!.w, side.avatar!.y - side.trigger!.y],
    };
  };
  expect(browse.trigger).not.toBeNull();
  expect(stand(work)).toEqual(stand(browse));
  screen.container.querySelector<HTMLElement>(".work-case")!.click();
  await settle();
  await expect
    .element(page.getByRole("menuitem", { name: "Sign in to Zephium", exact: true }))
    .toBeVisible();
  await expect.element(page.getByRole("menuitem", { name: /^Ada Lovelace/u })).toBeVisible();
  await shoot("foot-work-menu");
});
