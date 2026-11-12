import { expect, test, vi } from "vitest";
import { userEvent } from "vitest/browser";
import { render } from "vitest-browser-svelte";
import DialogHost from "./DialogHost.svelte";
vi.mock("$domain/surface", () => ({ holdChromeModal: () => () => {} }));

test("Escape preserves a pending dialog and failure leaves the retry visible", async () => {
  const screen = await render(DialogHost);
  const dialog = screen.getByRole("dialog", { name: "Clear site data" });
  await expect.element(dialog).toBeVisible();
  await userEvent.keyboard("{Escape}");
  await expect.element(dialog).toBeVisible();
  await screen.getByRole("button", { name: "Finish with failure" }).click();
  await expect.element(screen.getByRole("alert")).toHaveTextContent("Clear failed. Try again.");
  await expect.element(dialog).toBeVisible();
  await userEvent.keyboard("{Escape}");
  await expect.element(dialog).not.toBeVisible();
});
