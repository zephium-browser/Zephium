import test from "node:test";
import assert from "node:assert/strict";
import { normalize } from "./normalize.mjs";
test("volatile origins and identities normalize while relationships and ordering survive", () => {
  const make = (port, tab, doc) => [
    {
      label: "sender",
      value: {
        tab: { id: tab, index: 0, windowId: 7 },
        frameId: 0,
        documentId: doc,
        url: `http://127.0.0.1:${port}/a`,
      },
    },
    {
      label: "event",
      value: { tabId: tab, documentId: doc, frameId: 4, timeStamp: 123 },
    },
    { label: "other", value: { tabId: tab + 1 } },
  ];
  assert.deepEqual(
    normalize(make(8000, 10, "aaa"), {
      origins: { a: "http://127.0.0.1:8000" },
    }),
    normalize(make(9000, 20, "bbb"), {
      origins: { a: "http://127.0.0.1:9000" },
    }),
  );
  const actual = normalize(make(8000, 10, "aaa"));
  assert.equal(actual[0].value.tab.id, actual[1].value.tabId);
  assert.notEqual(actual[1].value.tabId, actual[2].value.tabId);
  assert.equal(actual[0].value.frameId, 0);
});
test("ordinary values, exact error punctuation, rule IDs and frame sentinels remain", () => {
  assert.deepEqual(
    normalize({
      id: 22,
      priority: 3,
      frameId: -1,
      error: "QUOTA_BYTES quota exceeded.",
      value: 45,
    }),
    {
      error: "QUOTA_BYTES quota exceeded.",
      frameId: -1,
      id: 22,
      priority: 3,
      value: 45,
    },
  );
});
test("normalization does not sort event arrays or mutate inputs", () => {
  const input = { z: [{ event: "second" }, { event: "first" }], a: 2 };
  const snapshot = structuredClone(input);
  assert.deepEqual(normalize(input).z, input.z);
  assert.deepEqual(input, snapshot);
});
test("known error-template identities retain their relations and exact surrounding text", () => {
  const result = normalize([
    { tabId: 99, windowId: 88 },
    { message: "No tab with id: 99." },
    { message: "No frame with id 2147483647 in tab with id 99" },
    { message: "No window with id: 88." },
  ]);
  assert.equal(result[1].message, `No tab with id: ${result[0].tabId}.`);
  assert.equal(
    result[2].message,
    `No frame with id 2147483647 in tab with id ${result[0].tabId}`,
  );
  assert.equal(result[3].message, `No window with id: ${result[0].windowId}.`);
});
test("scheme upgrades retain their scheme distinction after port normalization", () => {
  assert.deepEqual(
    normalize(["http://127.0.0.1:8080/a", "https://127.0.0.1:8080/a"], {
      origins: { a: "http://127.0.0.1:8080" },
    }),
    ["<origin:a>/a", "<origin:a:https>/a"],
  );
});
