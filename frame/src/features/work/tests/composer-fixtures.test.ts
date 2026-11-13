import { beforeEach, expect, test, vi } from "vitest";
import type { WorkComposerDraft, WorkResponseV1 } from "$shared/ipc/bindings";
import { mockComposerBindings } from "./composer-fixtures";

const native = vi.hoisted(() => ({ call: vi.fn() }));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockComposerBindings } = await import("./composer-fixtures");
  return mockComposerBindings({ workCall: native.call });
});
vi.mock("$shared/lib/close", () => ({ registerCloseTask: () => () => {} }));
vi.mock("$shared/ipc/native-events", () => ({ events: {} }));
beforeEach(() => {
  native.call.mockReset();
  native.call.mockImplementation(async (profile: string) => ({
    version: 1,
    profile,
    reply: { kind: "error", error: "not_found" },
  }));
});

function draft(response: WorkResponseV1): WorkComposerDraft {
  expect(response.reply.kind).toBe("environment");
  if (response.reply.kind !== "environment" || response.reply.reply.kind !== "composer")
    throw new Error("Expected scoped composer response");
  return response.reply.reply.draft;
}

test("the component fixture lets the production session prepare durable words before submission", async () => {
  const { WorkEnvironmentSession } = await import("$domain/work-environment");
  const session = new WorkEnvironmentSession("profile", "space");
  session.composer = "Compare the quiet keyboards";
  const prepared = await session.prepareComposer();
  expect(prepared).toMatchObject({ text: "Compare the quiet keyboards", revision: "2" });
  if (!prepared) throw new Error("Composer must be ready for submission");
  expect(await session.clearComposer(prepared)).toBe(true);
  expect(session.composer).toBe("");
  session.dispose();
});

test("composer calls preserve exact scope and reject stale revisions without replacing words", async () => {
  const fallback = vi.fn();
  const { commands } = mockComposerBindings({ workCall: fallback });
  const saved = draft(
    await commands.workCall("profile", {
      kind: "environment",
      version: 1,
      request: { kind: "composer_save", space: "space", expected: "1", text: "Kept words" },
    }),
  );
  expect(saved).toEqual({ profile: "profile", space: "space", revision: "2", text: "Kept words" });
  const stale = await commands.workCall("profile", {
    kind: "environment",
    version: 1,
    request: { kind: "composer_save", space: "space", expected: "1", text: "Stale words" },
  });
  expect(stale.reply).toEqual({ kind: "error", error: "conflict" });
  for (const [profile, space, text] of [
    ["profile", "space", "Kept words"],
    ["profile", "other", ""],
    ["other", "space", ""],
  ] as const) {
    expect(
      draft(
        await commands.workCall(profile, {
          kind: "environment",
          version: 1,
          request: { kind: "composer_read", space: space },
        }),
      ),
    ).toMatchObject({ profile, space, text });
  }
  expect(fallback).not.toHaveBeenCalled();
});

test("same-text saves and clears advance revisions and capacity failures do not acknowledge", async () => {
  const { commands } = mockComposerBindings({ workCall: vi.fn() });
  for (const expected of ["1", "2"]) {
    const saved = draft(
      await commands.workCall("profile", {
        kind: "environment",
        version: 1,
        request: { kind: "composer_save", space: "space", expected, text: "" },
      }),
    );
    expect(saved.revision).toBe(String(BigInt(expected) + 1n));
  }
  const refused = await commands.workCall("profile", {
    kind: "environment",
    version: 1,
    request: {
      kind: "composer_save",
      space: "space",
      expected: "3",
      text: "x".repeat(64 * 1024 + 1),
    },
  });
  expect(refused.reply).toEqual({ kind: "error", error: "capacity" });
  expect(
    draft(
      await commands.workCall("profile", {
        kind: "environment",
        version: 1,
        request: { kind: "composer_read", space: "space" },
      }),
    ),
  ).toMatchObject({ text: "", revision: "3" });
});

test("other Work calls and unexpected commands retain the suite's strict behavior", async () => {
  const failure = new Error("unhandled environment request");
  const fallback = vi.fn().mockRejectedValue(failure);
  const { commands } = mockComposerBindings({ workCall: fallback });
  const call = {
    kind: "environment",
    version: 1,
    request: { kind: "list", space: "space", after: null, limit: 20 },
  } as const;
  await expect(commands.workCall("profile", call)).rejects.toBe(failure);
  expect(fallback).toHaveBeenCalledExactlyOnceWith("profile", call);
  expect(() => commands.workOperation).toThrow("Unmocked native command: workOperation");
});

const isolated = mockComposerBindings({ workCall: vi.fn() }).commands;
test.each(["first", "second"])("%s test starts with fresh fixture rows", async () => {
  const call = {
    kind: "environment",
    version: 1,
    request: { kind: "composer_read", space: "shared" },
  } as const;
  expect(draft(await isolated.workCall("profile", call))).toMatchObject({
    revision: "1",
    text: "",
  });
  await isolated.workCall("profile", {
    kind: "environment",
    version: 1,
    request: { kind: "composer_save", space: "shared", expected: "1", text: "Previous case" },
  });
});
