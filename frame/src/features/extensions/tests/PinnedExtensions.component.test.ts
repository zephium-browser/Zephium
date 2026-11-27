import { afterEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import "$styles/global.css";
import PinnedExtensions from "../components/PinnedExtensions.svelte";

const native = vi.hoisted(() => ({
  actions: [] as unknown[],
  pinned: [] as string[],
  invoke: vi.fn(async (..._args: unknown[]) => {}),
}));
vi.mock("$domain/webext", () => ({
  webext: {
    byInstall: (install: string) => ({
      name: `Extension ${install}`,
      pinned: native.pinned.includes(install),
    }),
    MAX_PINNED: 3,
  },
}));
vi.mock("$domain/tabs", () => ({
  tabs: { profile: () => ({ id: "profile" }), activeId: () => "tab" },
}));
vi.mock("$domain/extensions", () => ({
  extensions: {
    activeActions: () => native.actions,
    isInvoking: () => false,
    invoke: native.invoke,
    menu: vi.fn(),
  },
}));

const action = (id: string) => ({
  runtime: { install_id: id, generation: "g" },
  revision: "r1",
  label: `Extension ${id}`,
  badge: id === "a" ? "3" : "",
  unread_badge: false,
  enabled: true,
  presents_popup: true,
  icon_rgba_base64: null,
});

afterEach(() => vi.clearAllMocks());

test("only pinned extensions get a button, three at most, badges included", async () => {
  native.actions = ["a", "b", "c", "d", "e"].map(action);
  native.pinned = ["a", "c", "d", "e"];
  const screen = await render(PinnedExtensions);
  const buttons = screen.container.querySelectorAll("button[data-pinned-extension]");
  expect([...buttons].map((button) => button.getAttribute("data-pinned-extension"))).toEqual([
    "a",
    "c",
    "d",
  ]);
  await expect.element(screen.getByRole("button", { name: "Extension a" })).toHaveTextContent("3");
  await screen.getByRole("button", { name: "Extension c" }).click();
  expect(native.invoke).toHaveBeenCalledExactlyOnceWith(
    "profile",
    "tab",
    native.actions[2],
    expect.any(DOMRect),
  );
});

test("nothing pinned draws nothing", async () => {
  native.actions = [action("a")];
  native.pinned = [];
  const screen = await render(PinnedExtensions);
  expect(screen.container.querySelector(".pins")).toBeNull();
});
