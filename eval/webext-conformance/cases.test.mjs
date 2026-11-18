import test from "node:test";
import assert from "node:assert/strict";
import { readFile, readdir } from "node:fs/promises";
const root = new URL("./", import.meta.url);
test("every case is self-describing MV3 with an unambiguous checked reference", async () => {
  const names = (await readdir(new URL("cases/", root))).sort();
  assert.ok(names.length >= 40 && names.length <= 60);
  const references = (await readdir(new URL("reference/", root)))
    .filter((name) => name.endsWith(".json"))
    .map((name) => name.slice(0, -5))
    .sort();
  assert.deepEqual(references, names);
  for (const name of names) {
    assert.match(name, /^[a-z][a-z0-9-]+$/);
    const metadata = JSON.parse(
      await readFile(new URL(`cases/${name}/case.json`, root), "utf8"),
    );
    const manifest = JSON.parse(
      await readFile(new URL(`cases/${name}/manifest.json`, root), "utf8"),
    );
    const reference = JSON.parse(
      await readFile(new URL(`reference/${name}.json`, root), "utf8"),
    );
    assert.equal(metadata.id, name);
    assert.ok(metadata.title.length > 5);
    assert.ok(metadata.apis.length > 0);
    assert.deepEqual(
      [...metadata.permissions].sort(),
      [...(manifest.permissions || [])].sort(),
      name,
    );
    assert.equal(manifest.manifest_version, 3);
    assert.equal(reference.caseId, name);
    assert.equal(reference.status, "complete");
    assert.ok(reference.observations.length > 0);
    assert.equal(reference.engine.playwrightVersion, "1.63.0");
    assert.equal(reference.engine.chromiumRevision, "1243");
  }
});
