import "$styles/global.css";
import { afterEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page } from "vitest/browser";
import ClearBrowsingData from "../components/ClearBrowsingData.svelte";

const native = vi.hoisted(() => ({ call: vi.fn() }));

vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({ browsingDataCall: native.call as never });
});
vi.mock("$domain/tabs", () => ({ tabs: { profile: () => ({ id: "profile" }) } }));

afterEach(() => native.call.mockReset());

async function open() {
  const screen = await render(ClearBrowsingData);
  await screen.getByRole("button", { name: "Clear…" }).click();
  return { screen, dialog: page.getByRole("dialog") };
}

test("the dialog offers four kinds with history, cookies and cache chosen", async () => {
  const { dialog } = await open();

  await expect.element(dialog.getByRole("checkbox")).toHaveLength(4);
  await expect.element(dialog.getByRole("checkbox", { name: "Browsing history" })).toBeChecked();
  await expect
    .element(dialog.getByRole("checkbox", { name: "Cookies and site data" }))
    .toBeChecked();
  await expect
    .element(dialog.getByRole("checkbox", { name: "Cached images and files" }))
    .toBeChecked();
  await expect.element(dialog.getByRole("checkbox", { name: "Download list" })).not.toBeChecked();
  await expect.element(dialog.getByLabelText("Time range")).toHaveTextContent("Last hour");
});

test("clearing sends the chosen range and exactly the chosen kinds", async () => {
  native.call.mockResolvedValue({ kind: "cleared" });
  const { dialog } = await open();

  await dialog.getByLabelText("Time range").click();
  await page.getByRole("option", { name: "Last 4 weeks" }).click();
  await dialog.getByRole("checkbox", { name: "Cached images and files" }).click();
  await dialog.getByRole("checkbox", { name: "Download list" }).click();
  await dialog.getByRole("button", { name: "Clear data" }).click();

  expect(native.call).toHaveBeenCalledExactlyOnceWith("profile", {
    kind: "clear",
    range: "four_weeks",
    history: true,
    cookies: true,
    cache: false,
    downloads: true,
  });
  await expect.element(page.getByRole("status")).toHaveTextContent("Browsing data cleared.");
  await expect.element(page.getByRole("dialog")).not.toBeInTheDocument();
});

test("every time range is offered", async () => {
  const { dialog } = await open();
  await dialog.getByLabelText("Time range").click();
  for (const name of ["Last hour", "Last 24 hours", "Last 7 days", "Last 4 weeks", "All time"])
    await expect.element(page.getByRole("option", { name })).toBeVisible();
});

test("time range options receive pointer input above the dialog", async () => {
  const { dialog } = await open();
  await dialog.getByLabelText("Time range").click();
  const option = page.getByRole("option", { name: "All time", exact: true });
  await expect.element(option).toBeVisible();
  await vi.waitFor(() => {
    const row = option.element();
    const box = row.getBoundingClientRect();
    expect(
      row.contains(document.elementFromPoint(box.x + box.width / 2, box.y + box.height / 2)),
    ).toBe(true);
  });
  await option.click();
  await expect.element(dialog.getByLabelText("Time range")).toHaveTextContent("All time");
  await expect.element(dialog).toBeVisible();
  expect(native.call).not.toHaveBeenCalled();
});

test("nothing chosen leaves nothing to clear", async () => {
  const { dialog } = await open();
  for (const name of ["Browsing history", "Cookies and site data", "Cached images and files"])
    await dialog.getByRole("checkbox", { name }).click();

  await expect.element(dialog.getByRole("button", { name: "Clear data" })).toBeDisabled();
  expect(native.call).not.toHaveBeenCalled();
});

test.each([
  [
    "the engine did not finish",
    () => native.call.mockResolvedValue({ kind: "error", error: "failed" }),
  ],
  [
    "the engine was unavailable",
    () => native.call.mockResolvedValue({ kind: "error", error: "unavailable" }),
  ],
  ["the call was lost", () => native.call.mockRejectedValue(new Error("offline"))],
])("a clear that fails because %s keeps the dialog open and says so", async (_, fail) => {
  fail();
  const { screen, dialog } = await open();
  await dialog.getByRole("button", { name: "Clear data" }).click();
  await expect.element(page.getByRole("alert")).toHaveTextContent("Couldn't clear everything");
  await expect.element(page.getByRole("dialog")).toBeVisible();
  expect(screen.container.textContent).not.toContain("Browsing data cleared.");
});

test("a private window says it has nothing to clear", async () => {
  native.call.mockResolvedValue({ kind: "error", error: "private" });
  const { dialog } = await open();
  await dialog.getByRole("button", { name: "Clear data" }).click();
  await expect
    .element(page.getByRole("alert"))
    .toHaveTextContent("A private window keeps nothing to clear.");
});
