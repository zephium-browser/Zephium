import { isDeepStrictEqual } from "node:util";
import { readFile, readdir } from "node:fs/promises";
import { resolve, join } from "node:path";
import { pathToFileURL } from "node:url";
export function differences(expected, actual, path = "") {
  if (isDeepStrictEqual(expected, actual)) return [];
  if (
    expected &&
    actual &&
    typeof expected === "object" &&
    typeof actual === "object" &&
    Array.isArray(expected) === Array.isArray(actual)
  ) {
    const keys = [
      ...new Set([...Object.keys(expected), ...Object.keys(actual)]),
    ].sort();
    return keys.flatMap((key) =>
      differences(
        expected[key],
        actual[key],
        `${path}/${key.replaceAll("~", "~0").replaceAll("/", "~1")}`,
      ),
    );
  }
  return [
    {
      path: path || "/",
      expected: expected === undefined ? { $missing: true } : expected,
      actual: actual === undefined ? { $missing: true } : actual,
    },
  ];
}
export function compare(
  reference,
  actual,
  allowlist = [],
  { sameEngine = false } = {},
) {
  if (
    !Array.isArray(actual.observations) ||
    !actual.observations.length ||
    !Array.isArray(reference.observations) ||
    !reference.observations.length ||
    actual.status !== "complete" ||
    reference.status !== "complete" ||
    actual.caseId !== reference.caseId ||
    actual.schemaVersion !== reference.schemaVersion
  )
    return {
      status: "fail",
      differences: [
        {
          path: "/status",
          expected: "complete matching case/schema",
          actual: actual.status,
        },
      ],
    };
  const changes = differences(
    reference.observations,
    actual.observations,
    "/observations",
  );
  if (sameEngine)
    changes.push(...differences(reference.engine, actual.engine, "/engine"));
  const applicable = allowlist.filter(
    (entry) =>
      entry.caseId === reference.caseId && entry.engine === actual.engine.name,
  );
  for (const entry of allowlist) {
    if (
      typeof entry.caseId !== "string" ||
      !entry.caseId.trim() ||
      typeof entry.engine !== "string" ||
      !entry.engine.trim() ||
      typeof entry.reason !== "string" ||
      !entry.reason.trim() ||
      typeof entry.source !== "string" ||
      !entry.source.trim() ||
      !entry.path?.startsWith("/observations/") ||
      !Object.hasOwn(entry, "expected") ||
      !Object.hasOwn(entry, "actual")
    )
      throw new Error(
        "Deviation needs case, engine, exact observation path, values, reason and source",
      );
  }
  const unallowed = changes.filter(
    (change) =>
      !applicable.some(
        (entry) =>
          entry.path === change.path &&
          isDeepStrictEqual(entry.expected, change.expected) &&
          isDeepStrictEqual(entry.actual, change.actual),
      ),
  );
  const stale = applicable.filter(
    (entry) =>
      !changes.some(
        (change) =>
          change.path === entry.path &&
          isDeepStrictEqual(change.expected, entry.expected) &&
          isDeepStrictEqual(change.actual, entry.actual),
      ),
  );
  return {
    status:
      unallowed.length || stale.length
        ? "fail"
        : changes.length
          ? "deviation"
          : "pass",
    differences: changes,
    stale,
  };
}
export async function compareDirectories(
  referenceDir,
  actualDir,
  allowlist,
  sameEngine = false,
) {
  const names = [
    ...new Set([
      ...(await readdir(referenceDir)),
      ...(await readdir(actualDir)),
    ]),
  ]
    .filter((name) => name.endsWith(".json"))
    .sort();
  let failed = false;
  console.log("| Case | Result | Differences |");
  console.log("| --- | --- | --- |");
  for (const name of names) {
    try {
      const reference = JSON.parse(
        await readFile(join(referenceDir, name), "utf8"),
      );
      const actual = JSON.parse(await readFile(join(actualDir, name), "utf8"));
      const result = compare(reference, actual, allowlist, { sameEngine });
      console.log(
        `| ${reference.caseId} | ${result.status} | ${result.differences.length}${result.stale?.length ? `; ${result.stale.length} stale deviations` : ""} |`,
      );
      if (result.status === "fail") {
        failed = true;
        console.error(JSON.stringify({ caseId: reference.caseId, ...result }));
      }
    } catch (error) {
      failed = true;
      console.log(`| ${name} | fail | ${error.message} |`);
    }
  }
  return !failed;
}
if (import.meta.url === pathToFileURL(resolve(process.argv[1] || "")).href) {
  const [referenceDir, actualDir, allowlistFile] = process.argv.slice(2);
  if (!referenceDir || !actualDir)
    throw new Error("Usage: node diff.mjs REFERENCE ACTUAL [ALLOWLIST]");
  const allowlist = JSON.parse(
    await readFile(
      allowlistFile || new URL("./deviations.json", import.meta.url),
      "utf8",
    ),
  );
  if (!(await compareDirectories(referenceDir, actualDir, allowlist)))
    process.exitCode = 1;
}
