import "$styles/global.css";
import { tick } from "svelte";
import { expect, test, vi } from "vitest";
import { userEvent } from "vitest/browser";
import { render } from "vitest-browser-svelte";
import { profiles } from "$domain/profiles";
import type { ProfileEntryView } from "$shared/ipc/bindings";
import ProfileSheet from "../components/profiles/ProfileSheet.svelte";

vi.mock("$domain/profiles", async (original) => {
  const actual = await original<typeof import("$domain/profiles")>();
  return {
    ...actual,
    profiles: { ...actual.profiles, rename: vi.fn(), setAvatar: vi.fn(), remove: vi.fn() },
  };
});

const work: ProfileEntryView = {
  id: "work",
  name: "Work",
  kind: "named",
  avatar: { style: "initials", color: 1, symbol: 0, revision: 0 },
};
const school: ProfileEntryView = { ...work, id: "school", name: "School" };

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

test("a save applies the submitted avatar to the original profile after reopening", async () => {
  const rename = deferred<boolean>();
  vi.mocked(profiles.rename).mockReturnValueOnce(rename.promise);
  vi.mocked(profiles.setAvatar).mockResolvedValueOnce(true);
  const screen = await render(ProfileSheet, { open: true, profile: work });
  await screen.getByLabelText("Name").fill("Projects");
  await screen.getByRole("button", { name: "Color 3", exact: true }).click();
  await screen.getByRole("button", { name: "Save", exact: true }).click();
  expect(profiles.rename).toHaveBeenCalledWith(work.id, "Projects");

  await userEvent.keyboard("{Escape}");
  await expect.element(screen.getByRole("dialog")).not.toBeInTheDocument();
  await screen.rerender({ open: true, profile: school });
  await expect.element(screen.getByLabelText("Name")).toHaveValue("School");
  await screen.getByRole("button", { name: "Color 4", exact: true }).click();
  rename.resolve(true);

  await expect
    .poll(() => vi.mocked(profiles.setAvatar).mock.calls[0])
    .toEqual([work.id, { style: "initials", color: 2, symbol: 0, photo: null }]);
  await expect.element(screen.getByRole("dialog")).toHaveAttribute("data-state", "open");
});

test("a completed deletion does not close a different profile's editor", async () => {
  const deletion = deferred<Awaited<ReturnType<typeof profiles.remove>>>();
  vi.mocked(profiles.remove).mockReturnValueOnce(deletion.promise);
  const screen = await render(ProfileSheet, { open: true, profile: work });
  await screen.getByRole("button", { name: "Delete Profile…" }).click();
  await screen.getByRole("button", { name: "Delete Profile", exact: true }).click();
  expect(profiles.remove).toHaveBeenCalledWith(work.id, undefined);

  await userEvent.keyboard("{Escape}");
  await expect.element(screen.getByRole("dialog")).not.toBeInTheDocument();
  await screen.rerender({ open: true, profile: school });
  await expect.element(screen.getByLabelText("Name")).toHaveValue("School");
  deletion.resolve({ state: "completed" });
  await deletion.promise;
  await tick();
  await expect.element(screen.getByRole("dialog")).toHaveAttribute("data-state", "open");
});
