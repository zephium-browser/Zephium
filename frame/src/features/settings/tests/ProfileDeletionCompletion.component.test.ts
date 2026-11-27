import "$styles/global.css";
import { afterEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { operations } from "$domain/operations";
import type { ProfileEntryView } from "$shared/ipc/bindings";
import { readDraft, writeDraft } from "$shared/lib/drafts";
import { emitNativeEvent } from "$shared/testing/native-events";
import ProfileSheet from "../components/profiles/ProfileSheet.svelte";
import ProfilesPage from "../components/sections/ProfilesPage.svelte";

const operationId = "0000000000000d01";
const workDeletion = "0000000000000d03";
const workProfile: ProfileEntryView = {
  id: "work-profile",
  name: "Work",
  kind: "named",
  avatar: { style: "initials", color: 1, symbol: 0, revision: 0 },
};
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    profilesGet: async () => ({
      status: "ok" as const,
      data: { current: "personal", private: false, profiles: [workProfile] },
    }),
    profilesDelete: async (id: string) => ({
      accepted: true,
      operation_id: id === workProfile.id ? workDeletion : operationId,
    }),
    operationStatus: async (operation_id: string) => ({ state: "pending" as const, operation_id }),
    operationAcknowledge: async () => true,
    operationsReconcile: async () => [],
  });
});
afterEach(() => {
  vi.useRealTimers();
  operations.dispose();
  localStorage.clear();
});

test("completed deletion erases profile drafts after its pending sheet closes and unmounts", async () => {
  await operations.init();
  writeDraft("note", ["deleted-profile", "space"], "private words");
  writeDraft("note", ["survivor-profile", "space"], "keep these words");
  const screen = await render(ProfileSheet, {
    open: true,
    profile: {
      id: "deleted-profile",
      name: "Work",
      kind: "named",
      avatar: { style: "initials", color: 1, symbol: 0, revision: 0 },
    },
  });
  await screen.getByRole("button", { name: "Delete Profile…" }).click();
  vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "Date", "performance"] });
  await screen.getByRole("button", { name: "Delete Profile", exact: true }).click();
  await vi.advanceTimersByTimeAsync(5000);
  vi.useRealTimers();
  await expect.element(screen.getByRole("status")).toHaveTextContent("Deletion is still pending");
  await screen.getByRole("button", { name: "Close", exact: true }).click();
  await screen.unmount();
  expect(readDraft("note", ["deleted-profile", "space"])).toBe("private words");
  emitNativeEvent("operationProcessed", {
    operation_id: "0000000000000d02",
    outcome: "applied",
    reason: "profile_deletion_completed",
  });
  expect(readDraft("note", ["deleted-profile", "space"])).toBe("private words");
  emitNativeEvent("operationProcessed", {
    operation_id: operationId,
    outcome: "applied",
    reason: "profile_deletion_completed",
  });
  expect(readDraft("note", ["deleted-profile", "space"])).toBeNull();
  expect(readDraft("note", ["survivor-profile", "space"])).toBe("keep these words");
});

test("Profiles confirms a late deletion after the person closes the pending dialog", async () => {
  await operations.init();
  const screen = await render(ProfilesPage);
  await screen.getByRole("button", { name: "Edit Work" }).click();
  await screen.getByRole("button", { name: "Delete Profile…" }).click();
  vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "Date", "performance"] });
  await screen.getByRole("button", { name: "Delete Profile", exact: true }).click();
  await vi.advanceTimersByTimeAsync(5000);
  vi.useRealTimers();
  await expect
    .element(screen.getByRole("dialog").getByRole("status"))
    .toHaveTextContent("Deletion is still pending");
  await screen.getByRole("button", { name: "Close", exact: true }).click();
  await expect.element(screen.getByRole("dialog")).not.toBeInTheDocument();
  await expect.element(screen.getByRole("status")).toHaveTextContent(/^$/);

  emitNativeEvent("operationProcessed", {
    operation_id: workDeletion,
    outcome: "applied",
    reason: "profile_deletion_completed",
  });
  await expect.element(screen.getByRole("status")).toHaveTextContent("Profile deleted.");
});
