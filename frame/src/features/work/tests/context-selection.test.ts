import { describe, expect, test } from "vitest";
import { contextSelection, formatBytes, referencesCanvasBlock } from "../lib/context-selection";
import type { WorkEnvironmentSnapshot, WorkRuntimeProjection } from "$shared/ipc/bindings";
import { tabFixture } from "$shared/testing/fixtures";

const snapshot: WorkEnvironmentSnapshot = {
  version: 1,
  id: "env",
  profile: "p",
  space: "s",
  title: "T",
  lifecycle: "active",
  revision: "3",
  elements: [
    { id: "e-note", area: null, reference: { kind: "resource", resource: "note-1" } },
    { id: "e-tab", area: null, reference: { kind: "browser", tab: "tab-1" } },
    { id: "e-obj", area: null, reference: { kind: "objective", objective: "work-1" } },
    {
      id: "e-art",
      area: null,
      reference: { kind: "artifact", objective: "work-1", execution: "x", artifact: "art-9" },
    },
    { id: "e-unknown", area: null, reference: { kind: "resource", resource: "note-missing" } },
  ],
  areas: [],
  view: { revision: "1", x: 0, y: 0, zoom_milli: 1000, placements: [] },
};

describe("contextSelection", () => {
  test("carries the identity token Rust recomputes for each selected element", () => {
    const objectives = new Map<string, WorkRuntimeProjection>([
      ["work-1", { work: { revision: "7" } } as unknown as WorkRuntimeProjection],
    ]);
    const selection = contextSelection(
      snapshot,
      ["e-note", "e-tab", "e-obj", "e-art", "e-unknown", "not-an-element"],
      [tabFixture({ id: "tab-1", url: "https://docs.example/a" })],
      {
        notes: [
          {
            id: "note-1",
            revision: "12",
            title: "N",
            preview: "",
            pinned: false,
            trashed: false,
            editable: true,
            created_at: "",
            modified_at: "",
            path: "N.md",
          },
        ],
        objectives,
        urls: new Map([["https://docs.example/a", "d1"]]),
      },
    );
    expect(selection).toEqual({
      environment: "env",
      items: [
        { element: "e-note", revision: "12" },
        { element: "e-tab", revision: "d1" },
        { element: "e-obj", revision: "7" },
        { element: "e-art", revision: "art-9" },
      ],
    });
  });

  test("an image or a document is context by its media record's revision", () => {
    const selection = contextSelection(snapshot, ["e-unknown"], [], {
      notes: [],
      objectives: new Map(),
      media: new Map([["note-missing", "4"]]),
    });
    expect(selection).toEqual({
      environment: "env",
      items: [{ element: "e-unknown", revision: "4" }],
    });
  });

  test("a tab is context by its address's digest, never the address itself", () => {
    const url = `https://outlook.live.com/mail/0/inbox/id/${"A".repeat(200)}`;
    const tabs = [tabFixture({ id: "tab-1", url })];
    const known = { notes: [], objectives: new Map() };
    expect(contextSelection(snapshot, ["e-tab"], tabs, known)).toBeNull();
    expect(
      contextSelection(snapshot, ["e-tab"], tabs, { ...known, urls: new Map([[url, "d2"]]) }),
    ).toEqual({ environment: "env", items: [{ element: "e-tab", revision: "d2" }] });
  });

  test("is null without a snapshot, a selection, or any resolvable token", () => {
    expect(contextSelection(null, ["e-note"], [], { notes: [], objectives: new Map() })).toBeNull();
    expect(contextSelection(snapshot, [], [], { notes: [], objectives: new Map() })).toBeNull();
    expect(
      contextSelection(snapshot, ["e-unknown"], [], { notes: [], objectives: new Map() }),
    ).toBeNull();
  });

  test("consent to open tabs stands alone or rides with a selection", () => {
    const known = { notes: [], objectives: new Map() };
    expect(contextSelection(snapshot, [], [], known, true)).toEqual({
      environment: "env",
      items: [],
      tabs: true,
    });
    expect(contextSelection(snapshot, ["e-unknown"], [], known, true)).toEqual({
      environment: "env",
      items: [],
      tabs: true,
    });
  });

  test("formats sizes for the manifest", () => {
    expect(formatBytes(512)).toBe("512 B");
    expect(formatBytes(2048)).toBe("2.0 KB");
    expect(formatBytes(20 * 1024)).toBe("20 KB");
  });
});

test("implicit block context requires a concrete reference and carries no unrelated items", () => {
  const known = { notes: [], objectives: new Map() };
  expect(referencesCanvasBlock("Summarize the news")).toBe(false);
  expect(referencesCanvasBlock("What is a calendar?")).toBe(false);
  expect(referencesCanvasBlock("Append lunch to my calendar")).toBe(true);
  expect(referencesCanvasBlock("Update this table and my checklist")).toBe(false);
  expect(contextSelection(snapshot, [], [], known, false, false)).toBeNull();
  expect(contextSelection(snapshot, ["e-unknown"], [], known, false, true)).toBeNull();
  expect(contextSelection(snapshot, [], [], known, false, true)).toEqual({
    environment: "env",
    items: [],
    blocks: true,
  });
  expect(
    contextSelection(snapshot, ["e-unknown"], [], {
      ...known,
      objects: new Map([["note-missing", "44"]]),
    }),
  ).toEqual({ environment: "env", items: [{ element: "e-unknown", revision: "44" }] });
});
