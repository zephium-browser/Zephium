import { afterEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page } from "vitest/browser";
import ExportBrowserData from "../components/ExportBrowserData.svelte";

const native = vi.hoisted(() => ({ export: vi.fn() }));

vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({ exportBrowserData: native.export as never });
});

afterEach(() => native.export.mockReset());

test("each row asks native to export its own kind", async () => {
  native.export.mockResolvedValue("saved");
  await render(ExportBrowserData);

  await page.getByRole("button", { name: "Export…" }).first().click();
  expect(native.export).toHaveBeenLastCalledWith("bookmarks");
  await expect.element(page.getByRole("status")).toHaveTextContent("Saved.");

  await page.getByRole("button", { name: "Export…" }).last().click();
  expect(native.export).toHaveBeenLastCalledWith("history");
});

test("closing the save dialog says nothing", async () => {
  native.export.mockResolvedValue("cancelled");
  await render(ExportBrowserData);

  await page.getByRole("button", { name: "Export…" }).first().click();

  await vi.waitFor(() => expect(native.export).toHaveBeenCalled());
  await expect.element(page.getByRole("status")).not.toBeInTheDocument();
  await expect.element(page.getByRole("alert")).not.toBeInTheDocument();
});

test("a private window and a failed save each explain themselves", async () => {
  native.export.mockResolvedValueOnce("private").mockResolvedValueOnce("failed");
  await render(ExportBrowserData);

  await page.getByRole("button", { name: "Export…" }).first().click();
  await expect
    .element(page.getByRole("status"))
    .toHaveTextContent("A private window keeps no bookmarks or history to export.");

  await page.getByRole("button", { name: "Export…" }).last().click();
  await expect.element(page.getByRole("alert")).toHaveTextContent("Couldn’t save the file");
  await expect.element(page.getByRole("status")).not.toBeInTheDocument();
});

test("only one export runs at a time", async () => {
  let finish: (value: string) => void = () => {};
  native.export.mockReturnValue(new Promise((done) => (finish = done)));
  await render(ExportBrowserData);

  await page.getByRole("button", { name: "Export…" }).first().click();
  await expect.element(page.getByRole("button", { name: "Export…" }).last()).toBeDisabled();
  expect(native.export).toHaveBeenCalledTimes(1);

  finish("saved");
  await expect.element(page.getByRole("button", { name: "Export…" }).last()).toBeEnabled();
});
