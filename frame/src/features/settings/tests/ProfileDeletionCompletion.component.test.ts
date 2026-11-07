import "$styles/global.css";
import { afterEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { operations } from "$domain/operations";
import { readDraft, writeDraft } from "$shared/lib/drafts";
import { emitNativeEvent } from "$shared/testing/native-events";
import ProfileSheet from "../components/profiles/ProfileSheet.svelte";

const operationId = "0000000000000d01";
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    profilesDelete: async () => ({ accepted: true, operation_id: "0000000000000d01" }),
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
