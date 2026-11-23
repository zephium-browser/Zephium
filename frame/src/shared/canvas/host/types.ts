import type { Edge, Node, SelectionMode, SvelteFlowProps } from "@xyflow/svelte";
import type { Viewport } from "../camera";
import type { Direction, FocusStop } from "../focus";

/**
 * What a top turns on. With none, the host takes no drag, no selection and
 * no marquee; a toolbar, a zoom level, areas, resizing and agent follow are
 * drawn by the top itself, from `$shared/canvas/capabilities` or its own code.
 */
export type CanvasCapabilities<N extends Node = Node, E extends Edge = Edge> = {
  /** Items move under the pointer. */
  drag?: {
    threshold: number;
    autoPan: boolean;
    onstart?: SvelteFlowProps<N, E>["onnodedragstart"];
    onstop?: SvelteFlowProps<N, E>["onnodedragstop"];
  };
  /** Items are selected, and several with a key held. */
  select?: {
    multiKey: string[];
    elevate: boolean;
    onchange?: SvelteFlowProps<N, E>["onselectionchange"];
  };
  /** A drag over the canvas draws a box that selects what it covers. */
  marquee?: {
    /** The box is drawn by a plain drag, not only with `key` held. */
    active: boolean;
    key: string;
    mode?: SelectionMode;
    onstart?: (event: PointerEvent) => void;
    onend?: (event: PointerEvent) => void;
  };
};

/** Arrow keys move between stops by where they stand, Enter acts on one, Escape leaves the canvas. */
export type CanvasKeyboard = {
  /** Every stop, drawn or not, where the layout puts it. */
  stops: () => readonly FocusStop[];
  /** The canvas's name, and what it is, for a screen reader. */
  label: string;
  role?: string;
  /** Chrome over the canvas's top and bottom: a stop is revealed clear of it. */
  inset?: { top: number; bottom: number };
  /** The top takes a key on a stop itself, before the host moves on. */
  onkey?: (id: string, direction: Direction) => boolean;
  onmove?: (id: string) => void;
  /** Enter on a stop that is not a button or a link. */
  onactivate?: (id: string) => void;
  onleave?: () => void;
};

export type CanvasCamera = {
  /** One eased move of the view, which replaces any still running. */
  glide: (view: Viewport, ms: number) => void;
  /** Puts the keyboard on a stop, bringing it into view first. */
  focus: (id: string) => void;
};
