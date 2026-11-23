import type { Node } from "@xyflow/svelte";
import type { Point } from "./lines";

/** Where an item stands on the canvas, resolving one level of parenting. */
export function absolutePosition(node: Node, nodes: readonly Node[]): Point {
  if (!node.parentId) return { ...node.position };
  const parent = nodes.find((candidate) => candidate.id === node.parentId);
  return parent
    ? { x: node.position.x + parent.position.x, y: node.position.y + parent.position.y }
    : { ...node.position };
}
