import type { Measure } from "./layout";

const HELD = 2000;

/** Measures text in the font it is drawn in, where a wrap depends on it. */
export function textMeasure(family: string): Measure | undefined {
  if (typeof document === "undefined") return undefined;
  const context = document.createElement("canvas").getContext("2d");
  if (!context) return undefined;
  const font = family || "system-ui";
  const widths = new Map<string, number>();
  return (text, size, weight = size >= 14 ? 600 : 500) => {
    const key = `${size}/${weight}/${text}`;
    const held = widths.get(key);
    if (held !== undefined) return held;
    context.font = `${weight} ${size}px ${font}`;
    const width = context.measureText(text).width;
    if (widths.size > HELD) widths.clear();
    widths.set(key, width);
    return width;
  };
}
