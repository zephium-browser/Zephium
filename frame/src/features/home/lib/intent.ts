import type { MapNetwork } from "$shared/ipc/bindings";

/** What the person did on Home's canvas, by ids only; the stage acts on it. */
export type HomeIntent =
  | { kind: "row"; search: string; row: string }
  | { kind: "inside"; search: string; row: string }
  | { kind: "fold"; search: string; row: string }
  | { kind: "site"; search: string; place: string }
  | { kind: "social"; search: string; place: string; network: MapNetwork }
  | { kind: "result"; search: string; result: string }
  | { kind: "page"; search: string; page: string }
  | { kind: "results"; search: string }
  | { kind: "address"; entry: string }
  | { kind: "forget"; search: string }
  | { kind: "again"; search: string }
  | { kind: "field" };
