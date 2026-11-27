import { expect, test, vi } from "vitest";
import { render } from "svelte/server";
import HidingBar from "../components/HidingBar.svelte";
vi.mock("$shared/platform", () => ({ IS_MAC: false }));
vi.mock("$domain/tabs", () => ({
  tabs: { activeId: () => "tab", activeTab: () => ({ url: "https://example.com" }) },
}));
vi.mock("$domain/blocker", () => ({
  hiding: {
    added: () => [{ label: "<script>page label</script>" }],
    isActive: () => true,
    isSaving: () => false,
    finish: () => {},
    undo: () => Promise.resolve(),
  },
}));
test("element hiding names the Windows undo keys and treats page labels as text", () => {
  const { body } = render(HidingBar);
  expect(body).toContain("Undo (Ctrl+Z)");
  expect(body).toContain("&lt;script>page label&lt;/script> hidden");
  expect(body).not.toContain("<script>page label");
});
