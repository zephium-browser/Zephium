import { afterEach, expect, test, vi } from "vitest";
import type { ArtifactContent } from "$shared/ui/data/Artifact";
import { diagramLayout, layoutDiagram } from "../lib/diagram";
import { engineLayout } from "../lib/diagram-worker";

vi.mock("../lib/diagram-worker", async (original) => {
  const actual = await original<typeof import("../lib/diagram-worker")>();
  return { ...actual, engineLayout: vi.fn(actual.engineLayout) };
});
afterEach(() => vi.unstubAllGlobals());

test("an evicted diagram asks for its settled layout again", async () => {
  type Diagram = Extract<ArtifactContent, { kind: "diagram" }>;
  const diagrams: Diagram[] = Array.from({ length: 49 }, (_, index) => ({
    kind: "diagram",
    layers: [],
    edges: [],
    nodes: [{ id: `n${index}`, name: `Node ${index}`, kind: "service" }],
  }));
  for (const diagram of diagrams) expect((await layoutDiagram(diagram)).settled).toBe(true);
  vi.stubGlobal("window", {});
  const calls = vi.mocked(engineLayout).mock.calls.length;
  expect(diagramLayout(diagrams[0]!).settled).toBe(false);
  await vi.waitFor(() => expect(engineLayout).toHaveBeenCalledTimes(calls + 1));
  await vi.waitFor(() => expect(diagramLayout(diagrams[0]!).settled).toBe(true));
});
