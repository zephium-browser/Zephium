import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const AsyncFunction = Object.getPrototypeOf(async function () {}).constructor;
const recover = new AsyncFunction(
  "expected",
  "modules",
  "navigator",
  "location",
  readFileSync(
    new URL(
      "../../crates/zephium-webext-macos/src/background/recover.js",
      import.meta.url,
    ),
    "utf8",
  ),
);
const expected = "chrome-extension://fixture/background/worker.js";

function fixture({
  scope = "chrome-extension://fixture/background/",
  scriptURL = expected,
  removed = true,
  finalState = "activated",
} = {}) {
  const calls = [];
  const worker = new EventTarget();
  worker.state = finalState;
  const registration = {
    scope,
    active: { scriptURL },
    async unregister() {
      calls.push("unregister");
      return removed;
    },
  };
  const navigator = {
    serviceWorker: {
      async getRegistration(url) {
        calls.push(["get", url]);
        return registration;
      },
      async register(url, options) {
        calls.push(["register", url, options]);
        return { active: worker };
      },
    },
  };
  return {
    calls,
    registration,
    navigator,
    worker,
    location: { href: expected },
  };
}

test("recovery preserves the worker URL, scope and classic/module type", async () => {
  for (const modules of [false, true]) {
    const f = fixture();
    assert.equal(
      await recover(expected, modules, f.navigator, f.location),
      true,
    );
    assert.deepEqual(f.calls.at(-1), [
      "register",
      expected,
      { scope: f.registration.scope, type: modules ? "module" : "classic" },
    ]);
  }
});

test("a different origin is refused before registration access", async () => {
  for (const href of [
    "https://fixture/background/worker.js",
    "chrome-extension://other/background/worker.js",
    "chrome-extension://fixture:81/background/worker.js",
  ]) {
    const f = fixture();
    await assert.rejects(
      recover(expected, false, f.navigator, { href }),
      /origin mismatch/,
    );
    assert.deepEqual(f.calls, []);
  }
});

test("a missing, different or cross-origin registration is never unregistered", async () => {
  for (const options of [
    { scriptURL: "chrome-extension://fixture/other.js" },
    { scope: "chrome-extension://other/" },
  ]) {
    const f = fixture(options);
    await assert.rejects(
      recover(expected, false, f.navigator, f.location),
      /registration mismatch/,
    );
    assert.equal(f.calls.includes("unregister"), false);
  }
  const f = fixture();
  f.navigator.serviceWorker.getRegistration = async () => undefined;
  await assert.rejects(
    recover(expected, false, f.navigator, f.location),
    /registration mismatch/,
  );
  assert.deepEqual(f.calls, []);
});

test("failed unregister does not create a second worker", async () => {
  const f = fixture({ removed: false });
  await assert.rejects(
    recover(expected, false, f.navigator, f.location),
    /unregister failed/,
  );
  assert.equal(
    f.calls.some((call) => Array.isArray(call) && call[0] === "register"),
    false,
  );
});

test("activation failure is not reported as success", async () => {
  const f = fixture({ finalState: "redundant" });
  await assert.rejects(
    recover(expected, true, f.navigator, f.location),
    /replacement failed/,
  );
});

test("recovery waits for activation instead of accepting an installing worker", async () => {
  const f = fixture({ finalState: "installing" });
  let settled = false;
  const result = recover(expected, false, f.navigator, f.location).then(
    (value) => {
      settled = true;
      return value;
    },
  );
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(settled, false);
  f.worker.state = "activated";
  f.worker.dispatchEvent(new Event("statechange"));
  assert.equal(await result, true);
});
