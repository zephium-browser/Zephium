import { expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { loadDocumentEditor } from "../index";

// Prepare the lazy dependency graph with the fixture imports. The test's
// budget measures draft ownership and teardown, not cold Vite transforms.
const { default: Editor } = await loadDocumentEditor();

test("publishes plain text drafts, preserves them across prop updates, and releases the editor", async () => {
  const onedit = vi.fn();
  const screen = await render(Editor, { paragraphs: ["<img src=x>"], label: "Draft", onedit });
  expect(screen.container.querySelector("img")).toBeNull();
  await screen.getByRole("textbox", { name: "Draft" }).fill("A local draft");
  expect(onedit).toHaveBeenLastCalledWith(["A local draft"]);
  await screen.rerender({ paragraphs: ["A stale projection"] });
  await expect.element(screen.getByRole("textbox")).toHaveTextContent("A local draft");
  await screen.rerender({ disabled: true });
  await expect.element(screen.getByRole("textbox")).toHaveAttribute("contenteditable", "false");
  await screen.unmount();
  expect(screen.container.querySelector(".tiptap")).toBeNull();
});
