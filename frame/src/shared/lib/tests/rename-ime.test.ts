import { expect, test, vi } from "vitest";
import { callback, callbackSource } from "$shared/testing/svelte-handlers";
import { isComposing } from "../keyboard";

const root = new URL("../../../features/", import.meta.url);
const handlers = [
  ["bookmarks/components/BookmarksPanel.svelte", "event", "onkeydown", "finishRename"],
  ["work/components/top/WorksMenu.svelte", "function", "renameKey", ""],
  ["work/components/AreaNode.svelte", "event", "onkeydown", "commit()"],
  ["work/components/compare/Compare.svelte", "event", "onkeydown", "commit(cell)"],
  ["tasks/components/TaskSubtasks.svelte", "event", "onkeydown", "field?.focus"],
] as const;

for (const [file, kind, name, contains] of handlers) {
  test(`${file} leaves candidate confirmation to the input method`, () => {
    const commit = vi.fn();
    const source = callbackSource(new URL(file, root), kind, name, contains)!;
    const run = callback<(event: object, row?: object) => void>(source, {
      isComposing,
      commit,
      commitRename: commit,
      finishRename: commit,
      field: { focus: commit },
      item: {},
      cell: {},
      renaming: null,
      editing: null,
    });
    for (const state of [
      { isComposing: true, keyCode: 13 },
      { isComposing: false, keyCode: 229 },
    ])
      run({ key: "Enter", ...state, preventDefault: vi.fn(), stopPropagation: vi.fn() }, {});
    expect(commit).not.toHaveBeenCalled();
    run(
      {
        key: "Enter",
        isComposing: false,
        keyCode: 13,
        preventDefault: vi.fn(),
        stopPropagation: vi.fn(),
      },
      {},
    );
    expect(commit).toHaveBeenCalledOnce();
  });
}
