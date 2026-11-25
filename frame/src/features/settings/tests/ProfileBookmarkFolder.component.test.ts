import "$styles/global.css";
import { beforeEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { profiles } from "$domain/profiles";
import ProfileSheet from "../components/profiles/ProfileSheet.svelte";

vi.mock("$domain/profiles", async (original) => {
  const actual = await original<typeof import("$domain/profiles")>();
  return {
    ...actual,
    profiles: {
      ...actual.profiles,
      current: () => ({
        id: "personal",
        name: "Personal",
        kind: "default",
        avatar: { style: "initials", color: 1, symbol: 0, revision: 0 },
      }),
      bookmarkFolders: vi.fn(),
      create: vi.fn(),
    },
  };
});

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(profiles.bookmarkFolders).mockResolvedValue([
    { id: "42", title: "Favorites" },
    { id: "57", title: "Reading" },
  ]);
  vi.mocked(profiles.create).mockResolvedValue(true);
});

test("a new profile can reuse an existing bookmarks folder", async () => {
  const screen = await render(ProfileSheet, { open: true });
  await expect.element(screen.getByRole("radio", { name: "Use existing folder" })).toBeEnabled();
  await screen.getByRole("radio", { name: "Use existing folder" }).click();
  await expect
    .element(screen.getByRole("combobox", { name: "Use existing folder" }))
    .toHaveValue("42");
  await screen.getByLabelText("Name").fill("Work");
  await screen.getByRole("button", { name: "Create Profile", exact: true }).click();
  await expect
    .poll(() => vi.mocked(profiles.create).mock.calls[0])
    .toEqual(["Work", { style: "initials", color: 1, symbol: 0, photo: null }, false, "42"]);
});

test("a new profile creates its own bookmarks folder by default", async () => {
  const screen = await render(ProfileSheet, { open: true });
  await expect.element(screen.getByRole("radio", { name: "Use existing folder" })).toBeEnabled();
  await screen.getByLabelText("Name").fill("Work");
  await screen.getByRole("button", { name: "Create Profile", exact: true }).click();
  await expect.poll(() => vi.mocked(profiles.create).mock.calls[0]?.[3]).toBeNull();
});
