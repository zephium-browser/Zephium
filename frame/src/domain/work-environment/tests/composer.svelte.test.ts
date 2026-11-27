import { readFileSync } from "node:fs";
import { runInNewContext } from "node:vm";
import { parse } from "svelte/compiler";
import { beforeEach, expect, test, vi } from "vitest";
import type { WorkComposerDraft, WorkEnvironmentCall, WorkReplyV1 } from "$shared/ipc/bindings";
import { WorkComposer } from "../composer.svelte";

const profile = "00000000000000000000000001";
const space = "00000000000000000000000002";
const reply = (draft: WorkComposerDraft): WorkReplyV1 => ({
  kind: "environment",
  reply: { kind: "composer", draft },
});
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}
function store() {
  let draft = { profile, space, revision: "1", text: "" };
  const call = vi.fn(async (request: WorkEnvironmentCall): Promise<WorkReplyV1> => {
    if (request.kind === "composer_save") {
      if (request.expected !== draft.revision) return { kind: "error", error: "conflict" };
      draft = { ...draft, revision: (BigInt(draft.revision) + 1n).toString(), text: request.text };
    }
    return reply(draft);
  });
  return { call, read: () => draft };
}
beforeEach(() => {
  vi.useRealTimers();
});

test("late hydration learns its revision without overwriting newly typed words", async () => {
  const read = deferred<WorkReplyV1>();
  const native = store();
  const call = vi.fn(native.call).mockImplementationOnce(() => read.promise);
  const composer = new WorkComposer(profile, space, call);
  composer.set("typed before the read settled");
  read.resolve(reply({ profile, space, revision: "1", text: "" }));
  expect(await composer.flush()).toBe(true);
  expect(composer.text).toBe("typed before the read settled");
  expect(native.read().text).toBe(composer.text);
});

test("competing documents cannot replace newer durable words or clear a later revision", async () => {
  const native = store();
  const first = new WorkComposer(profile, space, native.call);
  const second = new WorkComposer(profile, space, native.call);
  expect(await first.flush()).toBe(true);
  expect(await second.flush()).toBe(true);
  first.set("first saved words");
  const submitted = (await first.prepare())!;
  second.set("stale writer");
  expect(await second.flush()).toBe(false);
  expect(second.failure).toBe("conflict");
  const newest = new WorkComposer(profile, space, native.call);
  await newest.flush();
  newest.set("new durable words");
  expect(await newest.flush()).toBe(true);
  expect(await first.clear(submitted)).toBe(false);
  expect(native.read().text).toBe("new durable words");
  expect(first.text).toBe("first saved words");
});

test("submission flushes exact text and a late successful clear preserves new keystrokes", async () => {
  const native = store();
  const composer = new WorkComposer(profile, space, native.call);
  composer.set("  exact submitted text\n");
  const submitted = (await composer.prepare())!;
  expect(submitted.text).toBe("  exact submitted text\n");
  expect(native.read().text).toBe(submitted.text);
  const clearing = deferred<WorkReplyV1>();
  native.call.mockImplementationOnce(() => clearing.promise);
  const clear = composer.clear(submitted);
  composer.set("new keystrokes during clear");
  clearing.resolve(reply({ profile, space, revision: "3", text: "" }));
  expect(await clear).toBe(true);
  expect(composer.text).toBe("new keystrokes during clear");
  composer.forget();
});

test("failed native settlement preserves words, refuses close proof, and never claims saved", async () => {
  const native = store();
  const composer = new WorkComposer(profile, space, native.call);
  await composer.flush();
  composer.set("words that must stay");
  native.call.mockResolvedValueOnce({ kind: "error", error: "unavailable" });
  expect(await composer.prepare()).toBeNull();
  expect(composer.disposable).toBe(false);
  expect(composer.text).toBe("words that must stay");
  expect(composer.failure).toBe("unavailable");
  expect(native.read().text).toBe("");
  expect(await composer.flush()).toBe(true);
});

test("oversized text preserves the last durable text and malformed scoped replies fail closed", async () => {
  const native = store();
  const composer = new WorkComposer(profile, space, native.call);
  composer.set("last durable");
  await composer.flush();
  composer.set("x".repeat(65537));
  expect(await composer.flush()).toBe(false);
  expect(native.read().text).toBe("last durable");
  const malformed = new WorkComposer(profile, space, async () =>
    reply({ profile, space: "wrong", revision: "1", text: "foreign" }),
  );
  expect(await malformed.flush()).toBe(false);
  expect(malformed.text).toBe("");
  expect(malformed.failure).toBe("outcome_unknown");
  composer.forget();
});

test("retiring an owner rejects late hydration and cancels queued writes", async () => {
  vi.useFakeTimers();
  const read = deferred<WorkReplyV1>();
  const call = vi.fn(() => read.promise);
  const composer = new WorkComposer(profile, space, call);
  composer.set("about to be deleted");
  composer.forget();
  read.resolve(reply({ profile, space, revision: "1", text: "late old words" }));
  await vi.advanceTimersByTimeAsync(600);
  expect(composer.text).toBe("");
  expect(await composer.flush()).toBe(false);
  expect(call).toHaveBeenCalledTimes(1);
});

test("transport rejection can be retried and an unknown commit is reconciled without a duplicate overwrite", async () => {
  const native = store();
  const call = vi.fn(native.call).mockRejectedValueOnce(new Error("transport unavailable"));
  const composer = new WorkComposer(profile, space, call);
  composer.set("kept through failed hydration");
  expect(await composer.flush()).toBe(false);
  expect(await composer.resolve("retry")).toBe(true);
  const original = native.call.getMockImplementation()!;
  call.mockImplementationOnce(async (request) => {
    await original(request);
    throw new Error("reply lost after commit");
  });
  composer.set("committed but receipt lost");
  expect(await composer.flush()).toBe(false);
  const revision = native.read().revision;
  expect(await composer.resolve("retry")).toBe(true);
  expect(native.read().revision).toBe(revision);
  expect((await composer.prepare())?.text).toBe("committed but receipt lost");
});

test("a conflict offers explicit load or replacement without permanently blocking the document", async () => {
  const native = store();
  const first = new WorkComposer(profile, space, native.call);
  const stale = new WorkComposer(profile, space, native.call);
  await Promise.all([first.flush(), stale.flush()]);
  first.set("saved elsewhere");
  await first.flush();
  stale.set("my local words");
  expect(await stale.flush()).toBe(false);
  expect(await stale.resolve("retry")).toBe(false);
  expect(stale.text).toBe("my local words");
  expect(native.read().text).toBe("saved elsewhere");
  expect(await stale.resolve("replace")).toBe(true);
  expect(native.read().text).toBe("my local words");
  first.set("local version to explicitly discard");
  expect(await first.flush()).toBe(false);
  expect(await first.resolve("load")).toBe(true);
  expect(first.text).toBe("my local words");
  expect(await first.flush()).toBe(true);
});

test("prepare refuses a keystroke queued after the save receipt but before its continuation", async () => {
  const native = store();
  const composer = new WorkComposer(profile, space, native.call);
  await composer.flush();
  composer.set("persisted text");
  await composer.flush();
  vi.spyOn(composer, "flush").mockImplementation(() => {
    queueMicrotask(() => composer.set("new unacknowledged keystroke"));
    return Promise.resolve(true);
  });
  expect(await composer.prepare()).toBeNull();
  expect(composer.text).toBe("new unacknowledged keystroke");
  composer.forget();
});

test("retired composers are disposable and every later operation stays native-inert", async () => {
  const native = store();
  const composer = new WorkComposer(profile, space, native.call);
  await composer.flush();
  composer.set("deleted words");
  composer.forget();
  const count = native.call.mock.calls.length;
  expect(composer.disposable).toBe(true);
  expect(await composer.resolve("replace")).toBe(false);
  expect(await composer.resolve("load")).toBe(false);
  expect(await composer.prepare()).toBeNull();
  expect(native.call).toHaveBeenCalledTimes(count);
});

/** Execute the production Svelte handler with deterministic ports, without a browser. */
async function handler(file: string, name: string, ports: Record<string, unknown>) {
  const source = readFileSync(
    new URL(`../../../features/work/components/${file}.svelte`, import.meta.url),
    "utf8",
  );
  const program = parse(source, { modern: true }).instance?.content;
  const declaration = program?.body.find(
    (node) => node.type === "FunctionDeclaration" && node.id?.name === name,
  );
  if (
    !declaration ||
    !("start" in declaration) ||
    typeof declaration.start !== "number" ||
    !("end" in declaration) ||
    typeof declaration.end !== "number"
  )
    throw new Error(`Missing production handler ${file}.${name}`);
  await runInNewContext(`(${source.slice(declaration.start, declaration.end)})()`, ports);
}
for (const file of ["AgentLine", "WorkEnvironmentWorkspace"]) {
  test(`${file} preserves durable words on native steer refusal and clears only accepted words`, async () => {
    for (const accepted of [false, true]) {
      const native = store();
      const composer = new WorkComposer(profile, space, native.call);
      composer.set("  exact durable steering words  ");
      await composer.flush();
      const steer = vi.fn(async () => accepted);
      const enqueue = vi.fn(() => true);
      const clear = vi.fn((submitted: Awaited<ReturnType<typeof composer.prepare>>) =>
        composer.clear(submitted!),
      );
      const objective = { steer, enqueue };
      if (file === "AgentLine") {
        await handler(file, "steer", {
          session: objective,
          prepareDraft: async () => {
            const submitted = await composer.prepare();
            return submitted ? { text: submitted.text, clear: () => clear(submitted) } : null;
          },
        });
      } else {
        await handler(file, "send", {
          session: {
            get composer() {
              return composer.text;
            },
            prepareComposer: () => composer.prepare(),
            clearComposer: clear,
          },
          busy: false,
          blockSaveFailed: false,
          owned: { flushAll: async () => true },
          objectiveSession: objective,
          runningObjective: true,
          contextSel: null,
          openTabs: false,
          activeExecution: true,
          createObjective: () => {
            throw new Error("steer must not create another objective");
          },
        });
      }
      expect(steer).toHaveBeenCalledWith("exact durable steering words");
      expect(enqueue).not.toHaveBeenCalled();
      expect(clear).toHaveBeenCalledTimes(accepted ? 1 : 0);
      expect(native.read().text).toBe(accepted ? "" : "  exact durable steering words  ");
      expect(composer.text).toBe(native.read().text);
      composer.forget();
    }
  });
}

test("an unresolved clear attempt stays non-disposable even after its text was durably saved", async () => {
  const native = store();
  const composer = new WorkComposer(profile, space, native.call);
  composer.set("submitted words with an unknown clear outcome");
  const submitted = (await composer.prepare())!;
  native.call.mockResolvedValueOnce({ kind: "error", error: "outcome_unknown" });
  expect(await composer.clear(submitted)).toBe(false);
  await Promise.resolve();
  expect(composer.disposable).toBe(false);
  expect(composer.text).toBe(submitted.text);
  expect(await composer.resolve("retry")).toBe(true);
  expect(composer.text).toBe(submitted.text);
  expect(composer.disposable).toBe(true);
  composer.forget();
});
