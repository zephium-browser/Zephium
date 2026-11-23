<script lang="ts">
  import type { Node } from "@xyflow/svelte";
  import { CanvasHost, type CanvasKeyboard } from "../host";
  import type { FocusStop } from "..";
  import StopNode from "./StopNode.svelte";
  let { keyboard }: { keyboard?: Omit<CanvasKeyboard, "stops" | "label"> & { off?: boolean } } =
    $props();
  const cards = [
    { id: "a", x: 0, y: 0, rows: ["a1", "a2"] },
    { id: "b", x: 0, y: 300, rows: [] },
    { id: "c", x: 3000, y: 64, rows: [] },
  ];
  let nodes = $state.raw<Node[]>(
    cards.map((card) => ({
      id: card.id,
      type: "stops",
      position: { x: card.x, y: card.y },
      width: 240,
      height: 32 * (card.rows.length + 1),
      data: { rows: card.rows },
    })),
  );
  const stops: FocusStop[] = cards.flatMap((card) => [
    { id: card.id, rect: { x: card.x, y: card.y, width: 240, height: 32 } },
    ...card.rows.map((row, index) => ({
      id: row,
      rect: { x: card.x, y: card.y + 32 * (index + 1), width: 240, height: 32 },
    })),
  ]);
</script>

<div class="stage">
  <CanvasHost
    bind:nodes
    edges={[]}
    nodeTypes={{ stops: StopNode }}
    viewport={{ x: 40, y: 40, zoom: 1 }}
    keyboard={keyboard?.off
      ? undefined
      : { ...keyboard, stops: () => stops, label: "Searches", role: "canvas" }}
  />
</div>

<style>
  .stage {
    inline-size: 800px;
    block-size: 600px;
  }
</style>
