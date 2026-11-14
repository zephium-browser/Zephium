import test from "node:test";
import assert from "node:assert/strict";
import { compare, differences } from "./diff.mjs";
const transcript = (value) => ({
  schemaVersion: 1,
  caseId: "fixture",
  engine: { name: "candidate", version: "1" },
  status: "complete",
  observations: [{ label: "result", value }],
});
const entry = {
  caseId: "fixture",
  engine: "candidate",
  path: "/observations/0/value",
  expected: 1,
  actual: 2,
  reason: "Specific observed engine limitation",
  source: "fixture evidence",
};
test("strict differ reports ordered values and missing keys", () => {
  assert.equal(compare(transcript(1), transcript(2)).status, "fail");
  assert.deepEqual(differences({ a: 1 }, {}), [
    { path: "/a", expected: 1, actual: { $missing: true } },
  ]);
  assert.notEqual(
    compare(transcript(["a", "b"]), transcript(["b", "a"])).status,
    "pass",
  );
});
test("exact documented deviation passes only its engine, case, path and values", () => {
  assert.equal(
    compare(transcript(1), transcript(2), [entry]).status,
    "deviation",
  );
  assert.equal(compare(transcript(1), transcript(3), [entry]).status, "fail");
  assert.equal(
    compare(transcript(1), transcript(2), [{ ...entry, caseId: "elsewhere" }])
      .status,
    "fail",
  );
  assert.equal(
    compare(transcript(1), transcript(2), [{ ...entry, engine: "elsewhere" }])
      .status,
    "fail",
  );
  assert.throws(() =>
    compare(transcript(1), transcript(2), [{ ...entry, reason: "" }]),
  );
});
test("stale allowances and incomplete cases fail rather than hiding regressions", () => {
  assert.equal(compare(transcript(1), transcript(1), [entry]).status, "fail");
  assert.equal(
    compare(transcript(1), { ...transcript(1), status: "error" }, [entry])
      .status,
    "fail",
  );
  assert.equal(
    compare(transcript(1), { ...transcript(1), caseId: "wrong" }).status,
    "fail",
  );
});
test("reference check pins engine metadata while cross-engine differ ignores it", () => {
  const reference = transcript(1),
    actual = { ...transcript(1), engine: { name: "other", version: "2" } };
  assert.equal(compare(reference, actual).status, "pass");
  assert.equal(
    compare(reference, actual, [], { sameEngine: true }).status,
    "fail",
  );
});
