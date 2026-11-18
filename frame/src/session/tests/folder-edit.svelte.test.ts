import { beforeEach, expect, it, vi } from "vitest";
import type { FolderIntent, SidebarNodeView } from "$shared/ipc/bindings";

const view = vi.hoisted(() => ({
  nodes: [] as SidebarNodeView[],
  space: "s1" as string | null,
  answers: [] as ((outcome: string) => void)[],
  folder: vi.fn(),
}));

vi.mock("$domain/tabs", () => ({
  tabs: {
    sidebarNodes: () => view.nodes,
    profile: () => ({ id: "p1", name: "P", kind: "default" }),
    activeSpaceId: () => view.space,
    folder: (intent: FolderIntent) => {
      view.folder(intent);
      return new Promise((resolve) =>
        view.answers.push((outcome) => resolve({ outcome, disposition: null })),
      );
    },
  },
}));

const folder = (id: string): SidebarNodeView => ({
  id,
  parent_id: null,
  section: "today",
  kind: { type: "folder", name: "Folder", color: null, collapsed: false },
});

const settled = () => new Promise((resolve) => setTimeout(resolve, 0));

beforeEach(() => {
  vi.resetModules();
  view.nodes = [folder("old")];
  view.space = "s1";
  view.answers = [];
  view.folder.mockReset();
});

it("hands Group into Folder to the sidebar while one is drawn", async () => {
  const edit = await import("../folder-edit.svelte");
  expect(edit.askGroup()).toBe(false);
  const grouped = vi.fn();
  const release = edit.handleGroup(grouped);
  expect(edit.askGroup()).toBe(true);
  expect(grouped).toHaveBeenCalledOnce();
  release();
  expect(edit.askGroup()).toBe(false);
});

it("makes one folder at a time, each opening its own row for its name", async () => {
  const edit = await import("../folder-edit.svelte");
  const first = edit.create("One");
  const second = edit.create("Two");
  await settled();
  expect(view.folder).toHaveBeenCalledTimes(1);

  view.answers.shift()!("applied");
  await settled();
  // Its folder is not projected yet, so the next create waits for it.
  expect(view.folder).toHaveBeenCalledTimes(1);
  view.nodes = [folder("old"), folder("one")];
  expect(edit.fresh("old")).toBe(false);
  expect(edit.fresh("one")).toBe(true);
  edit.claim("one");
  expect(edit.renamingId()).toBe("one");
  await first;

  await settled();
  expect(view.folder).toHaveBeenCalledTimes(2);
  view.answers.shift()!("applied");
  view.nodes = [folder("old"), folder("one"), folder("two")];
  await second;
  expect(edit.renamingId()).toBe("two");
});

it("does not open a folder of another space when the space changed during a create", async () => {
  const edit = await import("../folder-edit.svelte");
  const made = edit.create("Here");
  await settled();
  view.space = "s2";
  view.nodes = [folder("theirs")];
  view.answers.shift()!("applied");
  await made;
  expect(edit.fresh("theirs")).toBe(false);
  expect(edit.renamingId()).toBeNull();
});

it("lets the next create go when a refused one never brings a folder", async () => {
  const edit = await import("../folder-edit.svelte");
  const refused = edit.create("No");
  const next = edit.create("Yes");
  await settled();
  view.answers.shift()!("rejected");
  await refused;
  await settled();
  expect(view.folder).toHaveBeenCalledTimes(2);
  view.nodes = [folder("old"), folder("yes")];
  view.answers.shift()!("applied");
  await next;
  expect(edit.renamingId()).toBe("yes");
});
