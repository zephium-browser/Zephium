/** Lines drawn at most. A page's source can run to millions of lines once
 *  minified code is unfolded; past this the rest stays in the copy. */
export const MAX_LINES = 20_000;
/** Lines per block, so the browser can skip the blocks off screen. */
export const CHUNK = 200;

export type SourceLines = {
  /** The lines drawn, in blocks. */
  chunks: { start: number; lines: string[] }[];
  /** Every line the source has. */
  total: number;
  /** Fewer lines are drawn than the source has. */
  cut: boolean;
};

/** Splits source text on any line ending and groups what is drawn. */
export function sourceLines(text: string): SourceLines {
  const all = text.split(/\r\n|\r|\n/u);
  const shown = all.length > MAX_LINES ? all.slice(0, MAX_LINES) : all;
  const chunks: SourceLines["chunks"] = [];
  for (let start = 0; start < shown.length; start += CHUNK) {
    chunks.push({ start, lines: shown.slice(start, start + CHUNK) });
  }
  return { chunks, total: all.length, cut: shown.length < all.length };
}
