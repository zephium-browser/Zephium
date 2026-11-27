import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import "$styles/global.css";
import "$shared/ui/Menu/surface.css";
import ExtensionsSection from "../components/ExtensionsSection.svelte";

type Action = {
  runtime: { install_id: string; generation: string };
  revision: string;
  label: string;
  badge: string;
  unread_badge: boolean;
  enabled: boolean;
  presents_popup: boolean;
  icon_rgba_base64: string | null;
};

const native = vi.hoisted(() => ({
  available: true,
  actions: [] as unknown[],
  shortcut: null as null | {
    runtime: { install_id: string; generation: string };
    action_revision: string;
    projection_revision: string;
  },
  invoke: vi.fn(async (..._args: unknown[]) => {}),
  consume: vi.fn((_revision: string) => true),
  open: vi.fn(async (_page: string) => {}),
  pinned: [] as string[],
  setPinned: vi.fn(async (_id: string, _pinned: boolean) => {}),
}));
vi.mock("$domain/webext", () => ({
  webext: {
    isAvailable: () => native.available,
    byInstall: (install: string) => ({
      id: `ext-${install}`,
      name: `Extension ${install}`,
      pinned: native.pinned.includes(install),
    }),
    pinnedCount: () => native.pinned.length,
    MAX_PINNED: 3,
    setPinned: native.setPinned,
  },
}));
vi.mock("$domain/surface", () => ({ surface: { open: native.open } }));
vi.mock("$domain/tabs", () => ({
  tabs: { profile: () => ({ id: "profile" }), activeId: () => "tab" },
}));
vi.mock("$domain/extensions", () => ({
  extensions: {
    activeActions: () => native.actions,
    failureReason: () => null,
    isInvoking: () => false,
    actionShortcut: () => native.shortcut,
    consumeActionShortcut: native.consume,
    invoke: native.invoke,
  },
}));

const action = (id: string, over: Partial<Action> = {}): Action => ({
  runtime: { install_id: id, generation: "g" },
  revision: "r1",
  label: `Extension ${id}`,
  badge: "",
  unread_badge: false,
  enabled: true,
  presents_popup: true,
  icon_rgba_base64: null,
  ...over,
});

beforeEach(() => {
  native.available = true;
  native.actions = [];
  native.shortcut = null;
  native.pinned = [];
});
afterEach(() => vi.clearAllMocks());

test("without extensions there is nothing to draw, not even a hairline's worth", async () => {
  native.available = false;
  const screen = await render(ExtensionsSection);
  expect(screen.container.querySelector(".extensions")).toBeNull();
  await expect
    .element(screen.getByRole("button", { name: "Manage extensions" }))
    .not.toBeInTheDocument();
});

test("each extension's action is a button named for it, beside the way to manage them", async () => {
  native.actions = [action("a"), action("b", { enabled: false })];
  const screen = await render(ExtensionsSection);
  await expect.element(screen.getByRole("button", { name: "Extension a" })).toBeEnabled();
  await expect.element(screen.getByRole("button", { name: "Extension b" })).toBeDisabled();
  await expect.element(screen.getByRole("button", { name: "Manage extensions" })).toBeVisible();
});

test("with no actions the way to manage them stays", async () => {
  const screen = await render(ExtensionsSection);
  await expect.element(screen.getByRole("button", { name: "Manage extensions" })).toBeVisible();
  await expect.element(screen.getByRole("button", { name: /^Extension / })).not.toBeInTheDocument();
});

test("pressing an action opens it from the page's tab, and leaves the panel", async () => {
  native.actions = [action("a")];
  const screen = await render(ExtensionsSection);
  const tile = screen.getByRole("button", { name: "Extension a" });
  await expect.element(tile).toHaveAttribute("data-dismiss");
  await tile.click();
  expect(native.invoke).toHaveBeenCalledExactlyOnceWith(
    "profile",
    "tab",
    native.actions[0],
    expect.any(DOMRect),
  );
});

test("an extension's key opens its action without the panel ever being opened", async () => {
  native.actions = [action("a")];
  native.shortcut = {
    runtime: { install_id: "a", generation: "g" },
    action_revision: "r1",
    projection_revision: "p1",
  };
  await render(ExtensionsSection);
  await expect.poll(() => native.invoke.mock.calls.length).toBe(1);
  expect(native.consume).toHaveBeenCalledWith("p1");
});

test("Manage extensions opens the extensions page and leaves the panel", async () => {
  const screen = await render(ExtensionsSection);
  const manage = screen.getByRole("button", { name: "Manage extensions" });
  await expect.element(manage).toHaveAttribute("data-dismiss");
  await manage.click();
  expect(native.open).toHaveBeenCalledExactlyOnceWith("extensions");
});

// The panel's width at the default 240px column, less its own padding.
const panelWidth = "228px";

const rect = (element: Element) => element.getBoundingClientRect();

test("the way to manage them heads a row for each extension", async () => {
  native.actions = [action("a"), action("b"), action("c")];
  const screen = await render(ExtensionsSection);
  screen.container.style.inlineSize = panelWidth;
  const head = screen.container.querySelector<HTMLElement>(".row")!;
  const manage = screen.getByRole("button", { name: "Manage extensions" });
  await expect.element(manage).toHaveTextContent("Extensions");
  expect(head.offsetHeight).toBe(34);
  expect(Math.round(rect(manage.element().querySelector(".name")!).left - rect(head).left)).toBe(8);
  const items = [
    ...screen.container.querySelectorAll<HTMLElement>("button[data-extension-install]"),
  ];
  expect(items).toHaveLength(3);
  for (const [index, item] of items.entries()) {
    expect(rect(item).top).toBeGreaterThanOrEqual(rect(head).bottom);
    if (index > 0) expect(rect(item).top).toBeGreaterThanOrEqual(rect(items[index - 1]!).bottom);
    expect(item.textContent).toContain(`Extension ${["a", "b", "c"][index]}`);
  }
});

test("a row pins its extension to the address field, up to three", async () => {
  native.actions = [action("a"), action("b"), action("c"), action("d")];
  native.pinned = ["b", "c", "d"];
  const screen = await render(ExtensionsSection);
  await expect.element(screen.getByRole("button", { name: "Pin to sidebar" })).toBeDisabled();
  const unpin = screen.getByRole("button", { name: "Unpin from sidebar" }).first();
  await expect.element(unpin).toHaveAttribute("aria-pressed", "true");
  await unpin.click();
  expect(native.setPinned).toHaveBeenCalledExactlyOnceWith("ext-b", false);
});

test("many extensions stay inside the panel", async () => {
  native.actions = Array.from({ length: 9 }, (_, index) => action(`x${index}`));
  const screen = await render(ExtensionsSection);
  screen.container.style.inlineSize = panelWidth;
  await expect.element(screen.getByRole("button", { name: "Extension x8" })).toBeVisible();
  const box = rect(screen.container.querySelector(".extensions")!);
  for (const item of screen.container.querySelectorAll("button[data-extension-install]")) {
    expect(rect(item).left).toBeGreaterThanOrEqual(box.left);
    expect(rect(item).right).toBeLessThanOrEqual(box.right);
  }
});

test("each button has its own hover, and the others keep none", async () => {
  native.actions = [action("a"), action("b")];
  const screen = await render(ExtensionsSection);
  screen.container.style.inlineSize = panelWidth;
  const a = screen.getByRole("button", { name: "Extension a" });
  const b = screen.getByRole("button", { name: "Extension b" });
  const manage = screen.getByRole("button", { name: "Manage extensions" });
  // Whether a button has a plate of its own: the fill is not fully transparent.
  const lit = (locator: typeof a) => {
    const fill = getComputedStyle(locator.element()).backgroundColor;
    return Number(/,\s*([\d.]+)\)$/u.exec(fill)?.[1] ?? 1) > 0;
  };
  expect(lit(a) || lit(b) || lit(manage)).toBe(false);
  await a.hover();
  await expect.poll(() => lit(a)).toBe(true);
  expect(lit(b)).toBe(false);
  expect(lit(manage)).toBe(false);
  await manage.hover();
  await expect.poll(() => lit(manage)).toBe(true);
  await expect.poll(() => lit(a)).toBe(false);
  expect(lit(b)).toBe(false);
});

test("the keyboard reaches the manager and then each extension, each with a ring", async () => {
  native.actions = [action("a")];
  const screen = await render(ExtensionsSection);
  const manage = screen.getByRole("button", { name: "Manage extensions" });
  manage.element().focus({ focusVisible: true } as FocusOptions);
  await expect.element(manage).toHaveFocus();
  expect(getComputedStyle(manage.element()).outlineStyle).toBe("solid");
  const tile = screen.getByRole("button", { name: "Extension a" });
  tile.element().focus({ focusVisible: true } as FocusOptions);
  await expect.element(tile).toHaveFocus();
  expect(getComputedStyle(tile.element()).outlineStyle).toBe("solid");
});
