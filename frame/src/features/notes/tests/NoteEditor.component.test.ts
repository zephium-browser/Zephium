import { expect, test, vi } from "vitest";
import { page, userEvent } from "vitest/browser";
import { render } from "vitest-browser-svelte";
import type { NoteSummary } from "$domain/notes";
import { notesTestServer } from "$shared/testing/notes/server";
import NoteEditor from "../components/editor/NoteEditor.svelte";

test("a changed link query cannot select the previous query's note", async () => {
  const packing = notesTestServer("p").seed("# Packing list");
  const find = vi.fn((query: string): Promise<NoteSummary[]> =>
    query === "Pack" ? Promise.resolve([packing]) : new Promise(() => {}),
  );
  const screen = await render(NoteEditor, {
    source: "# Draft\n\nSee ",
    editable: true,
    onchange: vi.fn(),
    onopenlink: vi.fn(),
    onopennote: vi.fn(),
    resolve: async () => ({}),
    find,
  });
  const text = screen.getByRole("textbox", { name: "Note" });
  await text.getByText("See").click();
  await userEvent.keyboard("{End}[[[[Pack");
  await expect
    .element(page.getByRole("option", { name: "Packing list", exact: true }))
    .toBeVisible();

  await userEvent.keyboard("{Backspace}{Backspace}{Backspace}{Backspace}Trip");
  expect(document.querySelector('[role="option"]')?.textContent).not.toContain("Packing list");
  await expect.poll(() => find.mock.calls.some(([query]) => query === "Trip")).toBe(true);
  expect(document.querySelector('[role="option"]')?.textContent).not.toContain("Packing list");
  await userEvent.keyboard("{Enter}");
  expect(text.element().querySelector("[data-wiki-link]")?.getAttribute("data-wiki-link")).toBe(
    "Trip",
  );
});
