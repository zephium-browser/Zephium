import assert from "node:assert/strict";
import { readFile, readdir } from "node:fs/promises";
import { test } from "node:test";
import {
  inventory,
  omittedCalls,
  projectReference,
  supportedCases,
} from "./node-plan.mjs";

test("every oracle case has explicit Node coverage or an unavailability reason", async () => {
  const ids = await readdir(new URL("./cases", import.meta.url));
  const rows = inventory(ids);
  assert.equal(rows.length, 57);
  assert.equal(new Set(supportedCases).size, supportedCases.length);
  assert.deepEqual(
    rows
      .filter((row) => row.coverage !== "unavailable")
      .map((row) => row.id)
      .sort(),
    supportedCases,
  );
  for (const row of rows.filter((row) => row.coverage !== "full"))
    assert.ok(row.reason);
  assert.throws(() => inventory(["future-unreviewed-case"]), /Unclassified/);
});

test("registration projection explicitly omits only native injection observations", async () => {
  const id = "scripting-register-lifecycle";
  const reference = JSON.parse(
    await readFile(new URL(`./reference/${id}.json`, import.meta.url), "utf8"),
  );
  const original = JSON.stringify(reference);
  const projected = projectReference(reference, omittedCalls[id]);
  assert.equal(
    reference.observations.length - projected.observations.length,
    10,
  );
  assert.equal(JSON.stringify(reference), original);
  assert.ok(
    projected.observations.some((row) => row.label === "duplicate id:error"),
  );
  assert.ok(
    projected.observations.some(
      (row) => row.label === "update missing id:error",
    ),
  );
  assert.throws(
    () => projectReference(reference, ["unreviewed omission"]),
    /Missing or duplicate/,
  );
  assert.throws(
    () =>
      projectReference(
        {
          ...reference,
          observations: [
            ...reference.observations,
            reference.observations.find(
              (row) => row.label === "existing document is unchanged:call",
            ),
          ],
        },
        omittedCalls[id],
      ),
    /Missing or duplicate/,
  );
});
