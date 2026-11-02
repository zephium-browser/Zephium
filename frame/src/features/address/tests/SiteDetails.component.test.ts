import type { ComponentProps } from "svelte";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import "$styles/global.css";
import "$shared/ui/Menu/surface.css";
import type { SitePermissionKindView, SitePermissionsView } from "$shared/ipc/bindings";
import { clearRequest } from "$domain/site-data";
import SiteDetails from "../components/SiteDetails.svelte";
import type { SitePage } from "../lib/address-model";

const native = vi.hoisted(() => ({
  list: vi.fn<(origin?: string) => Promise<SitePermissionsView | null>>(),
  set: vi.fn(async (..._args: unknown[]) => true),
  commands: [] as string[],
}));
vi.mock("$domain/permissions", () => ({
  sitePermissions: { list: native.list, set: native.set },
}));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    runCommand: async (id: string) => {
      native.commands.push(id);
      return { accepted: true, operation_id: null };
    },
  });
});

const KINDS: SitePermissionKindView[] = ["camera", "microphone", "geolocation", "popups"];
const MEET = "https://meet.example/room";

function view(
  entries: SitePermissionsView["entries"] = [],
  over: Partial<SitePermissionsView> = {},
): SitePermissionsView {
  return { private: false, complete: true, kinds: KINDS, entries, ...over };
}

function pageOf(over: Partial<SitePage> = {}): SitePage {
  return {
    url: MEET,
    security: "secure",
    warning: null,
    capture: null,
    privateWindow: false,
    zoom: null,
    standardZoom: 100,
    ...over,
  };
}

beforeEach(() => {
  native.commands = [];
  native.list.mockResolvedValue(view());
});
afterEach(() => {
  vi.clearAllMocks();
  clearRequest.dismiss();
});

function details(props: Partial<ComponentProps<typeof SiteDetails>> = {}) {
  return render(SiteDetails, { page: pageOf(), open: true, ...props });
}

const CAMERA = { origin: "https://meet.example", kind: "camera", decision: "allow" } as const;
const GEO = { origin: "https://meet.example", kind: "geolocation", decision: "block" } as const;
const POPUPS = { origin: "https://meet.example", kind: "popups", decision: "allow" } as const;

test("closed, it loads nothing and reads nothing; opening loads it and reads once", async () => {
  native.list.mockResolvedValue(view([CAMERA]));
  const screen = await details({ open: false });
  await new Promise((resolve) => setTimeout(resolve, 50));
  expect(native.list).not.toHaveBeenCalled();
  await expect.element(screen.getByRole("radiogroup")).not.toBeInTheDocument();

  await screen.rerender({ page: pageOf(), open: true });
  await expect.element(screen.getByRole("radiogroup", { name: "Camera" })).toBeVisible();
  expect(native.list).toHaveBeenCalledExactlyOnceWith("https://meet.example");
});

test("it reads again each time it opens", async () => {
  const screen = await details();
  await expect.poll(() => native.list.mock.calls.length).toBe(1);
  await screen.rerender({ page: pageOf(), open: false });
  await screen.rerender({ page: pageOf(), open: true });
  await expect.poll(() => native.list.mock.calls.length).toBe(2);
});

test("a site with no decision and nothing in use shows no permissions, and says nothing about it", async () => {
  const screen = await details();
  await expect.poll(() => native.list.mock.calls.length).toBe(1);
  await expect
    .element(screen.getByRole("button", { name: "Clear data for this site…" }))
    .toBeVisible();
  expect(screen.container.querySelector("section")).toBeNull();
  expect(screen.container.textContent).not.toMatch(/permission/iu);
  expect(screen.container.textContent).not.toContain("No ");
});

test("each decision is a row with Allow and Block, and a press is written through and read back", async () => {
  native.list.mockResolvedValue(view([CAMERA, GEO]));
  const screen = await details();
  const camera = screen.getByRole("radiogroup", { name: "Camera" });
  await expect.element(camera.getByRole("radio", { name: "Allow" })).toBeChecked();
  await expect.element(camera.getByRole("radio", { name: "Block" })).not.toBeChecked();
  await expect
    .element(
      screen.getByRole("radiogroup", { name: "Location" }).getByRole("radio", { name: "Block" }),
    )
    .toBeChecked();
  await expect
    .element(screen.getByRole("radiogroup", { name: "Microphone" }))
    .not.toBeInTheDocument();
  expect(native.set).not.toHaveBeenCalled();

  await camera.getByRole("radio", { name: "Block" }).click();
  expect(native.set).toHaveBeenLastCalledWith("https://meet.example", "camera", "block");
  await expect.poll(() => native.list.mock.calls.length).toBe(2);
});

test("pressing the choice a row already has changes nothing", async () => {
  native.list.mockResolvedValue(view([CAMERA]));
  const screen = await details();
  await screen
    .getByRole("radiogroup", { name: "Camera" })
    .getByRole("radio", { name: "Allow" })
    .click();
  expect(native.set).not.toHaveBeenCalled();
});

test("pop-ups allowed, then blocked, forget the allowance and keep their row", async () => {
  native.list.mockResolvedValue(view([POPUPS]));
  const screen = await details();
  const group = screen.getByRole("radiogroup", { name: "Pop-ups & redirects" });
  await expect.element(group.getByRole("radio", { name: "Allow" })).toBeChecked();
  native.list.mockResolvedValue(view());
  await group.getByRole("radio", { name: "Block" }).click();
  expect(native.set).toHaveBeenLastCalledWith("https://meet.example", "popups", null);
  await expect.poll(() => native.list.mock.calls.length).toBe(2);
  await expect.element(group.getByRole("radio", { name: "Block" })).toBeChecked();
});

test("a camera in use has a row, marked, with no choice made yet", async () => {
  const screen = await details({
    page: pageOf({ capture: { camera: "active", microphone: "none" } }),
  });
  const camera = screen.getByRole("radiogroup", { name: "Camera" });
  await expect.element(camera).toBeVisible();
  await expect.element(camera.getByRole("radio", { name: "Allow" })).not.toBeChecked();
  await expect.element(camera.getByRole("radio", { name: "Block" })).not.toBeChecked();
  await expect.element(screen.getByText("In use")).toBeVisible();
  await expect
    .element(screen.getByRole("radiogroup", { name: "Microphone" }))
    .not.toBeInTheDocument();
});

test("a change that did not go through says so and shows what is saved", async () => {
  native.list.mockResolvedValue(view([CAMERA]));
  native.set.mockResolvedValueOnce(false);
  const screen = await details();
  const camera = screen.getByRole("radiogroup", { name: "Camera" });
  await camera.getByRole("radio", { name: "Block" }).click();
  await expect
    .element(screen.getByRole("alert"))
    .toHaveTextContent("Couldn't change this site's permission.");
  await expect.element(camera.getByRole("radio", { name: "Allow" })).toBeChecked();
});

test("Reset permissions forgets every decision of this site and the section goes with them", async () => {
  native.list.mockResolvedValueOnce(view([CAMERA, POPUPS])).mockResolvedValue(view());
  const screen = await details();
  await screen.getByRole("button", { name: "Reset permissions" }).click();
  await expect.poll(() => native.set.mock.calls.length).toBe(2);
  expect(native.set).toHaveBeenCalledWith("https://meet.example", "camera", null);
  expect(native.set).toHaveBeenCalledWith("https://meet.example", "popups", null);
  await expect.element(screen.getByRole("radiogroup")).not.toBeInTheDocument();
  await expect
    .element(screen.getByRole("button", { name: "Reset permissions" }))
    .not.toBeInTheDocument();
});

test("permissions that cannot be read are not shown as none", async () => {
  native.list.mockResolvedValue(null);
  const screen = await details();
  await expect.element(screen.getByText("Site settings are unavailable right now.")).toBeVisible();
  await expect.element(screen.getByRole("radiogroup")).not.toBeInTheDocument();
});

test("a partly unreadable list keeps its rows and says so", async () => {
  native.list.mockResolvedValue(view([POPUPS], { complete: false }));
  const screen = await details();
  await expect
    .element(screen.getByRole("radiogroup", { name: "Pop-ups & redirects" }))
    .toBeVisible();
  await expect
    .element(screen.getByText("Some permissions couldn't be read right now."))
    .toBeVisible();
});

test("a private window has no permissions to show and no data to clear, and reads nothing", async () => {
  const screen = await details({
    page: pageOf({ privateWindow: true, capture: { camera: "active", microphone: "none" } }),
  });
  await new Promise((resolve) => setTimeout(resolve, 50));
  expect(native.list).not.toHaveBeenCalled();
  expect(screen.container.querySelector("section")).toBeNull();
  await expect.element(screen.getByRole("radiogroup")).not.toBeInTheDocument();
  await expect
    .element(screen.getByRole("button", { name: "Clear data for this site…" }))
    .not.toBeInTheDocument();
});

test("zoom shows only away from the default, with its level, steps and Reset", async () => {
  const screen = await details({ page: pageOf({ zoom: 125 }) });
  const zoom = screen.getByRole("group", { name: "Page zoom" });
  await expect.element(zoom).toBeVisible();
  await expect.element(zoom.getByRole("status")).toHaveTextContent("125%");
  await zoom.getByRole("button", { name: "Zoom out" }).click();
  await zoom.getByRole("button", { name: "Zoom in" }).click();
  await zoom.getByRole("button", { name: "Reset", exact: true }).click();
  expect(native.commands).toEqual(["zoom.out", "zoom.in", "zoom.reset"]);
});

test("a page at the default zoom, or with none yet, shows no zoom", async () => {
  const at = await details({ page: pageOf({ zoom: 100 }) });
  await expect.element(at.getByRole("group", { name: "Page zoom" })).not.toBeInTheDocument();
  at.unmount();
  const none = await details({ page: pageOf({ zoom: null }) });
  await expect.element(none.getByRole("group", { name: "Page zoom" })).not.toBeInTheDocument();
});

test("the default zoom is what zoom is measured against", async () => {
  const screen = await details({ page: pageOf({ zoom: 125, standardZoom: 125 }) });
  await expect.poll(() => native.list.mock.calls.length).toBe(1);
  await expect.element(screen.getByRole("group", { name: "Page zoom" })).not.toBeInTheDocument();
});

test("stepping back to the default keeps the row until the panel closes", async () => {
  const screen = await details({ page: pageOf({ zoom: 110 }) });
  const zoom = screen.getByRole("group", { name: "Page zoom" });
  await expect.element(zoom).toBeVisible();
  await screen.rerender({ page: pageOf({ zoom: 100 }), open: true });
  await expect.element(zoom.getByRole("status")).toHaveTextContent("100%");
  await screen.rerender({ page: pageOf({ zoom: 100 }), open: false });
  await expect.element(zoom).not.toBeInTheDocument();
});

test("a row at the default zoom opens with no zoom row, and one zoomed meanwhile appears", async () => {
  const screen = await details({ page: pageOf({ zoom: 100 }) });
  const zoom = screen.getByRole("group", { name: "Page zoom" });
  await expect.element(zoom).not.toBeInTheDocument();
  await screen.rerender({ page: pageOf({ zoom: 150 }), open: true });
  await expect.element(zoom).toBeVisible();
});

test("Clear data asks the existing confirmation for this site", async () => {
  const screen = await details();
  await screen.getByRole("button", { name: "Clear data for this site…" }).click();
  expect(clearRequest.pending()).toEqual({ url: MEET, host: "meet.example" });
});

test("there is no link to Site settings and no explanatory text", async () => {
  native.list.mockResolvedValue(view([CAMERA]));
  const screen = await details({ page: pageOf({ zoom: 125 }) });
  await expect.element(screen.getByRole("radiogroup", { name: "Camera" })).toBeVisible();
  await expect
    .element(screen.getByRole("button", { name: "Site settings" }))
    .not.toBeInTheDocument();
  expect(screen.container.querySelectorAll("p")).toHaveLength(0);
});

// The panel's width at the default 240px column, less its own padding.
const panelWidth = "228px";

test("a permission is its name and its choice, with no icon; zoom and Clear data keep theirs on one axis", async () => {
  const kinds: SitePermissionKindView[] = [
    "camera",
    "microphone",
    "geolocation",
    "notifications",
    "popups",
    "automatic_downloads",
  ];
  native.list.mockResolvedValue(
    view(
      kinds.map((kind) => ({ origin: "https://meet.example", kind, decision: "allow" as const })),
      { kinds },
    ),
  );
  const screen = await details({ page: pageOf({ zoom: 125 }) });
  screen.container.style.inlineSize = panelWidth;
  await expect.element(screen.getByRole("radiogroup", { name: "Camera" })).toBeVisible();

  expect(screen.container.querySelectorAll("li svg")).toHaveLength(0);
  const icons = [...screen.container.querySelectorAll<HTMLElement>(".ui-menu-icon")];
  expect(icons).toHaveLength(2);
  for (const icon of icons) expect(icon.querySelector("svg")).not.toBeNull();
  expect(Math.round(icons[0]!.getBoundingClientRect().left)).toBe(
    Math.round(icons[1]!.getBoundingClientRect().left),
  );

  // The choice stays on its name's line: a name that is long takes two lines
  // of its own before the choice is asked to drop.
  for (const row of screen.container.querySelectorAll<HTMLElement>("li")) {
    const control = row.querySelector(".control")!.getBoundingClientRect();
    const name = row.querySelector(".name")!.getBoundingClientRect();
    expect(row.offsetHeight, row.textContent ?? "").toBeLessThanOrEqual(38);
    expect(control.left).toBeGreaterThanOrEqual(name.right);
    // Names start where the icons do.
    expect(Math.round(name.left)).toBe(Math.round(icons[0]!.getBoundingClientRect().left));
  }

  const zoom = screen.getByRole("group", { name: "Page zoom" }).element() as HTMLElement;
  expect(zoom.offsetHeight).toBe(34);
  const clear = screen.getByRole("button", { name: "Clear data for this site…" }).element();
  expect((clear as HTMLElement).offsetHeight).toBe(34);
});

test("the keyboard reaches Clear data and it shows a ring", async () => {
  const screen = await details();
  const clear = screen.getByRole("button", { name: "Clear data for this site…" });
  await expect.element(clear).toBeVisible();
  clear.element().focus({ focusVisible: true } as FocusOptions);
  await expect.element(clear).toHaveFocus();
  expect(getComputedStyle(clear.element()).outlineStyle).toBe("solid");
});

test("zoom takes its level as its name, so its controls and Reset share the row", async () => {
  const screen = await details({ page: pageOf({ zoom: 125 }) });
  screen.container.style.inlineSize = panelWidth;
  const zoom = screen.getByRole("group", { name: "Page zoom" });
  await expect.element(zoom).toBeVisible();
  const row = zoom.element() as HTMLElement;
  expect(row.offsetHeight).toBe(34);
  const level = row.querySelector("output")!.getBoundingClientRect();
  const controls = row.querySelector(".controls")!.getBoundingClientRect();
  expect(level.right).toBeLessThanOrEqual(controls.left);
  expect(row.scrollWidth).toBeLessThanOrEqual(row.clientWidth);
});
