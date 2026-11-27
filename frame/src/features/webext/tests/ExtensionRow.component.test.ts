import { afterEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page, userEvent } from "vitest/browser";
import "$styles/global.css";
import type { WebExtensionView } from "$shared/ipc/bindings";
import ExtensionRow from "../components/ExtensionRow.svelte";

const actions = vi.hoisted(() => ({
  setEnabled: vi.fn(async () => {}),
  setAccess: vi.fn(async () => {}),
  retry: vi.fn(async () => {}),
  reviewUpdate: vi.fn(async () => {}),
  openOptions: vi.fn(async () => {}),
  uninstall: vi.fn(async () => {}),
  requested: null as string | null,
  isRemovalRequested: vi.fn((id: string) => actions.requested === id),
  settleRemovalRequest: vi.fn(() => {
    actions.requested = null;
  }),
}));

vi.mock("$domain/webext", () => ({ webext: actions }));

afterEach(() => {
  vi.clearAllMocks();
  actions.requested = null;
});

function extension(overrides: Partial<WebExtensionView> = {}): WebExtensionView {
  return {
    profile_id: "profile-a",
    install_id: "install-a",
    id: "eimadpbcbfnmbkopoojfekhnkhdbieeh",
    name: "Dark Reader",
    version: "4.9.130",
    description: "",
    enabled: true,
    icon: null,
    state: "running",
    error: null,
    warnings: [],
    limitations: [],
    access: "all",
    sites: [],
    site_scoped: true,
    has_options: true,
    held_update: null,
    sideloaded: false,
    pinned: false,
    ...overrides,
  };
}

test("a running extension shows its version and site access", async () => {
  render(ExtensionRow, { extension: extension({ access: "sites", sites: ["github.com"] }) });
  await expect.element(page.getByText("Version 4.9.130")).toBeVisible();
  await expect.element(page.getByText("1 site")).toBeVisible();
});

test("an extension with limitations says so and lists them on hover", async () => {
  render(ExtensionRow, { extension: extension({ limitations: ["Can't manage downloads"] }) });
  const note = page.getByText("Some features unavailable");
  await expect.element(note).toBeVisible();
  await expect.element(note).toHaveAttribute("title", "Can't manage downloads");
});

test("a failed extension offers a retry", async () => {
  render(ExtensionRow, {
    extension: extension({ state: "failed", error: "The background crashed." }),
  });
  await expect.element(page.getByText(/Couldn't start: The background crashed\./)).toBeVisible();
  await userEvent.click(page.getByRole("button", { name: "Retry" }));
  expect(actions.retry).toHaveBeenCalledWith(
    expect.objectContaining({ profile_id: "profile-a", install_id: "install-a" }),
  );
});

test("an update waiting for approval can be reviewed", async () => {
  render(ExtensionRow, { extension: extension({ held_update: "4.10.0" }) });
  await expect.element(page.getByText(/Version 4\.10\.0 needs your approval/)).toBeVisible();
  await userEvent.click(page.getByRole("button", { name: "Review" }));
  expect(actions.reviewUpdate).toHaveBeenCalledWith("eimadpbcbfnmbkopoojfekhnkhdbieeh");
});

test("the switch turns the extension off", async () => {
  render(ExtensionRow, { extension: extension() });
  await userEvent.click(page.getByRole("switch", { name: "Turn Dark Reader on or off" }));
  expect(actions.setEnabled).toHaveBeenCalledWith("eimadpbcbfnmbkopoojfekhnkhdbieeh", false);
});

test("removing asks first", async () => {
  render(ExtensionRow, { extension: extension() });
  await userEvent.click(page.getByRole("button", { name: "More for Dark Reader" }));
  await userEvent.click(page.getByRole("menuitem", { name: "Remove" }));
  await expect.element(page.getByText("Remove Dark Reader and its data?")).toBeVisible();
  expect(actions.uninstall).not.toHaveBeenCalled();
  await userEvent.click(page.getByRole("button", { name: "Remove", exact: true }));
  expect(actions.uninstall).toHaveBeenCalledWith("eimadpbcbfnmbkopoojfekhnkhdbieeh");
});

test("Remove… chosen on the extension's toolbar button asks here, once", async () => {
  actions.requested = "eimadpbcbfnmbkopoojfekhnkhdbieeh";
  render(ExtensionRow, { extension: extension() });
  await expect.element(page.getByText("Remove Dark Reader and its data?")).toBeVisible();
  expect(actions.settleRemovalRequest).toHaveBeenCalledOnce();
  expect(actions.uninstall).not.toHaveBeenCalled();
  await userEvent.click(page.getByRole("button", { name: "Keep" }));
  await expect.element(page.getByText("Remove Dark Reader and its data?")).not.toBeInTheDocument();
});

test("a removal asked for another extension leaves this row alone", async () => {
  actions.requested = "another-extension";
  render(ExtensionRow, { extension: extension() });
  await expect.element(page.getByText("Version 4.9.130")).toBeVisible();
  expect(page.getByText("Remove Dark Reader and its data?").query()).toBeNull();
  expect(actions.settleRemovalRequest).not.toHaveBeenCalled();
});

test("specific sites are edited in place and saved as the user typed them", async () => {
  render(ExtensionRow, { extension: extension() });
  await userEvent.click(page.getByRole("button", { name: "More for Dark Reader" }));
  await userEvent.click(page.getByRole("menuitemcheckbox", { name: "On specific sites…" }));
  await userEvent.fill(page.getByRole("textbox", { name: "Add site" }), "github.com");
  await userEvent.keyboard("{Enter}");
  await userEvent.click(page.getByRole("button", { name: "Done" }));
  expect(actions.setAccess).toHaveBeenCalledWith("eimadpbcbfnmbkopoojfekhnkhdbieeh", "sites", [
    "github.com",
  ]);
});
