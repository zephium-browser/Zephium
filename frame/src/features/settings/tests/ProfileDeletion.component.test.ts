import "$styles/global.css";
import { beforeEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { profiles } from "$domain/profiles";
import ProfileSheet from "../components/profiles/ProfileSheet.svelte";

vi.mock("$domain/profiles", async (original) => {
  const actual = await original<typeof import("$domain/profiles")>();
  return { ...actual, profiles: { ...actual.profiles, remove: vi.fn() } };
});

beforeEach(() => vi.mocked(profiles.remove).mockReset());

test("pending deletion stays visible and checks the original operation", async () => {
  vi.mocked(profiles.remove)
    .mockResolvedValueOnce({ state: "pending", operation_id: "0000000000000001" })
    .mockResolvedValueOnce({ state: "completed" });
  const screen = await render(ProfileSheet, {
    open: true,
    profile: {
      id: "profile",
      name: "Work",
      kind: "named",
      avatar: { style: "initials", color: 1, symbol: 0, revision: 0 },
    },
  });
  await screen.getByRole("button", { name: "Delete Profile…" }).click();
  await screen.getByRole("button", { name: "Delete Profile", exact: true }).click();
  await expect.element(screen.getByRole("status")).toHaveTextContent("Deletion is still pending");
  await screen.getByRole("button", { name: "Check Deletion" }).click();
  expect(profiles.remove).toHaveBeenLastCalledWith("profile", "0000000000000001");
  await expect.element(screen.getByRole("dialog")).not.toBeInTheDocument();
});

test("pending deletion can be closed without offering cancellation", async () => {
  vi.mocked(profiles.remove).mockResolvedValue({
    state: "pending",
    operation_id: "0000000000000001",
  });
  const screen = await render(ProfileSheet, {
    open: true,
    profile: {
      id: "profile",
      name: "Work",
      kind: "named",
      avatar: { style: "initials", color: 1, symbol: 0, revision: 0 },
    },
  });
  await screen.getByRole("button", { name: "Delete Profile…" }).click();
  await screen.getByRole("button", { name: "Delete Profile", exact: true }).click();
  await expect.element(screen.getByRole("status")).toHaveTextContent("Deletion is still pending");
  await expect
    .element(screen.getByRole("button", { name: "Cancel", exact: true }))
    .not.toBeInTheDocument();
  await screen.getByRole("button", { name: "Close", exact: true }).click();
  await expect.element(screen.getByRole("dialog")).not.toBeInTheDocument();
  expect(profiles.remove).toHaveBeenCalledTimes(1);
});
