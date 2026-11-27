import { expect, test, vi } from "vitest";

const native = vi.hoisted(() => ({ folders: vi.fn() }));
vi.mock("$shared/ipc/bindings", () => ({
  commands: { profilesBookmarkFolders: native.folders },
}));

import { bookmarkFolders } from "../profiles.svelte";

test("unavailable bookmark folder lookups return an empty list", async () => {
  native.folders.mockRejectedValueOnce(new Error("unavailable"));
  await expect(bookmarkFolders("profile")).resolves.toEqual([]);
});
