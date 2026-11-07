import { afterEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page } from "vitest/browser";
import SiteData from "../components/SiteData.svelte";

const native = vi.hoisted(() => ({ call: vi.fn() }));

vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({ browsingDataCall: native.call as never });
});
vi.mock("$domain/tabs", () => ({ tabs: { profile: () => ({ id: "profile" }) } }));

afterEach(() => native.call.mockReset());

const sites = [
  { site: "example.com", cookies: true, storage: true },
  { site: "news.example.org", cookies: true, storage: false },
  { site: "app.test", cookies: false, storage: true },
];

function listing(partial = false) {
  native.call.mockResolvedValueOnce({ kind: "sites", sites, partial });
}

async function open() {
  const screen = await render(SiteData);
  await screen.getByRole("button", { name: "Manage…" }).click();
  return page.getByRole("dialog");
}

test("the dialog lists the sites with what each holds", async () => {
  listing();
  const dialog = await open();

  expect(native.call).toHaveBeenCalledExactlyOnceWith("profile", { kind: "sites" });
  await expect.element(dialog.getByText("example.com")).toBeVisible();
  await expect.element(dialog.getByRole("listitem")).toHaveLength(3);
  await expect
    .element(dialog.getByRole("listitem").nth(0))
    .toHaveTextContent("Cookies and site data");
  await expect.element(dialog.getByRole("listitem").nth(1)).toHaveTextContent("Cookies");
  await expect.element(dialog.getByRole("listitem").nth(2)).toHaveTextContent("Site data");
  expect(dialog.element().textContent).not.toContain("Only sites that stored cookies");
});

test("a list the engine can only give in part says so", async () => {
  listing(true);
  const dialog = await open();
  await expect.element(dialog.getByText("example.com")).toBeVisible();
  expect(dialog.element().textContent).toContain("Only sites that stored cookies are listed here.");
});

test("searching narrows the list and a miss says so", async () => {
  listing();
  const dialog = await open();

  await dialog.getByRole("searchbox", { name: "Search sites" }).fill("EXAMPLE");
  await expect.element(dialog.getByRole("listitem")).toHaveLength(2);

  await dialog.getByRole("searchbox", { name: "Search sites" }).fill("nothing here");
  await expect.element(dialog.getByText("No sites match your search.")).toBeVisible();
});

test("removing a site asks for that site only and drops its row", async () => {
  listing();
  native.call.mockResolvedValueOnce({ kind: "cleared" });
  const dialog = await open();

  await dialog.getByRole("button", { name: "Remove data for news.example.org" }).click();

  expect(native.call).toHaveBeenLastCalledWith("profile", {
    kind: "forget_site",
    site: "news.example.org",
  });
  await expect.element(dialog.getByRole("listitem")).toHaveLength(2);
  expect(dialog.element().textContent).not.toContain("news.example.org");
});

test("a site that could not be removed stays listed and says so", async () => {
  listing();
  native.call.mockResolvedValueOnce({ kind: "error", error: "failed" });
  const dialog = await open();

  await dialog.getByRole("button", { name: "Remove data for example.com" }).click();

  await expect
    .element(page.getByRole("alert"))
    .toHaveTextContent("Couldn't remove the data of example.com");
  await expect.element(dialog.getByRole("listitem")).toHaveLength(3);
});

test("removing everything asks first, then clears cookies and site data of all time", async () => {
  listing();
  native.call.mockResolvedValueOnce({ kind: "cleared" });
  native.call.mockResolvedValueOnce({ kind: "sites", sites: [], partial: false });
  const dialog = await open();

  await dialog.getByRole("button", { name: "Remove all", exact: true }).click();
  expect(native.call).toHaveBeenCalledOnce();
  await expect.element(dialog.getByText(/signed out of most of them/)).toBeVisible();

  await dialog.getByRole("button", { name: "Remove all data" }).click();

  expect(native.call).toHaveBeenNthCalledWith(2, "profile", {
    kind: "clear",
    range: "everything",
    history: false,
    cookies: true,
    cache: false,
    downloads: false,
  });
  await expect.element(dialog.getByText("No site data")).toBeVisible();
});

test("backing out of remove all removes nothing", async () => {
  listing();
  const dialog = await open();

  await dialog.getByRole("button", { name: "Remove all", exact: true }).click();
  await dialog.getByRole("button", { name: "Cancel" }).click();

  expect(native.call).toHaveBeenCalledOnce();
  await expect.element(dialog.getByRole("listitem")).toHaveLength(3);
});

test("a list that cannot be read offers to try again", async () => {
  native.call.mockResolvedValueOnce({ kind: "error", error: "unavailable" });
  listing();
  const dialog = await open();

  await expect.element(page.getByRole("alert")).toHaveTextContent("Couldn't list the sites");
  await dialog.getByRole("button", { name: "Try again" }).click();
  await expect.element(dialog.getByText("example.com")).toBeVisible();
});

test("no stored data is an empty state with nothing to remove", async () => {
  native.call.mockResolvedValueOnce({ kind: "sites", sites: [], partial: false });
  const dialog = await open();
  await expect.element(dialog.getByText("No site data")).toBeVisible();
  await expect
    .element(dialog.getByRole("button", { name: "Remove all", exact: true }))
    .not.toBeInTheDocument();
});
