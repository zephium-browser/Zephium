import { beforeEach } from "vitest";
import type { commands, WorkComposerDraft } from "$shared/ipc/bindings";
import { mockBindings } from "$shared/testing/bindings";

type WorkCall = typeof commands.workCall;
const stores = new Set<Map<string, WorkComposerDraft>>();
beforeEach(() => {
  for (const store of stores) store.clear();
});

/** Explicit fixture protocol: scoped native revisions, including empty tombstones. */
export function composerCall(fallback: WorkCall): WorkCall {
  const drafts = new Map<string, WorkComposerDraft>();
  stores.add(drafts);
  return async (profile, call) => {
    if (
      call.kind !== "environment" ||
      (call.request.kind !== "composer_read" && call.request.kind !== "composer_save")
    )
      return fallback(profile, call);
    const request = call.request;
    const key = JSON.stringify([profile, request.space]);
    const current = drafts.get(key) ?? {
      profile,
      space: request.space,
      text: "",
      revision: "1",
    };
    const error = (error: "conflict" | "capacity"): Awaited<ReturnType<WorkCall>> => ({
      version: 1,
      profile,
      reply: { kind: "error", error },
    });
    let draft = current;
    if (request.kind === "composer_save") {
      if (request.expected !== current.revision) return error("conflict");
      if (
        request.text.length > 64 * 1024 ||
        new TextEncoder().encode(request.text).length > 256 * 1024 ||
        BigInt(current.revision) === 9223372036854775807n
      )
        return error("capacity");
      draft = {
        ...current,
        text: request.text,
        revision: (BigInt(current.revision) + 1n).toString(),
      };
      drafts.set(key, draft);
    }
    return {
      version: 1,
      profile,
      reply: { kind: "environment", reply: { kind: "composer", draft: { ...draft } } },
    };
  };
}

/** Opt in without changing strict defaults or the suite's other native handlers. */
export function mockComposerBindings(
  overrides: Partial<typeof commands> & Pick<typeof commands, "workCall">,
) {
  return mockBindings({ ...overrides, workCall: composerCall(overrides.workCall) });
}
