import type { Notch } from "./ground";
import { WORDMARK } from "$shared/lib/wordmark";

/** A box in the page's own pixels. */
export type Box = { x: number; y: number; w: number; h: number };

export type Layout = {
  /** The field's notch, narrowed so both top corners stay free. */
  notch: Notch;
  /** The page's controls, each in its own round pane (a capsule when it
   *  holds words), in the top corner beside the notch and level with the
   *  field, the last nearest the corner. */
  controls: Box[];
  /** Where the sky meets the page. */
  horizon: number;
  /** The name, the sky's one large figure. */
  mark: Box;
  /** Where the line with the time and the date stands under the name, or
   *  null without a clock. */
  when: number | null;
};

const NOTCH: Notch = { width: 640, height: 50, radius: 18, flare: 14 };
const CORNER = 110;
const CONTROL = { size: 32, gap: 6, inset: 14 };
/** What two round controls take from the corner, which CORNER was set for. */
const TWO_ROUND = CONTROL.inset + 2 * CONTROL.size + CONTROL.gap;
/** The date's line, and what stands between the time, the name and it. */
/** The line with the time and the date, and its distance under the name. */
const WHEN = { gap: 16, height: 20 };

/** Everything on the page laid out from its size alone, so the sky that is
 *  cut and the things that stand in the cuts come from the same numbers in
 *  the same frame, and a resize can never draw one ahead of the other. */
export function layout(
  width: number,
  height: number,
  {
    controls,
    clock,
    wordmark = true,
  }: { controls: readonly number[]; clock: boolean; wordmark?: boolean },
): Layout {
  // Both top corners stay clear of the notch's flare for the controls in
  // one of them, so the field stays in the middle however wide they are.
  const reach = controls.reduce((sum, w) => sum + w + CONTROL.gap, CONTROL.inset - CONTROL.gap);
  const corner = Math.max(CORNER, reach + CORNER - TWO_ROUND);
  const notch = { ...NOTCH, width: Math.min(NOTCH.width, width - 2 * corner) };

  const buttonY = Math.round((NOTCH.height - CONTROL.size) / 2);
  let right = width - CONTROL.inset;
  const buttons = controls
    .toReversed()
    .map((w) => {
      const box = { x: right - w, y: buttonY, w, h: CONTROL.size };
      right = box.x - CONTROL.gap;
      return box;
    })
    .toReversed();

  // The sky takes the top of the page. The name stands low in the open sky,
  // where the clouds draw back toward the horizon, and the time and the
  // date read on one line under it.
  const horizon = Math.round(height * (height < 600 ? 0.62 : 0.6));
  const markW = Math.round(Math.min(480, Math.max(260, width * 0.36)));
  const markH = wordmark ? Math.ceil((markW * WORDMARK.height) / WORDMARK.width) : 0;
  const clockGap = wordmark ? WHEN.gap : 0;
  const group = markH + (clock ? clockGap + WHEN.height : 0);
  const centre = NOTCH.height + (horizon - NOTCH.height) * 0.74;
  const top = Math.max(NOTCH.height + 32, Math.round(centre - group / 2));
  const mark = { x: Math.round((width - markW) / 2), y: top, w: markW, h: markH };

  return {
    notch,
    controls: buttons,
    horizon,
    mark,
    when: clock ? top + markH + clockGap : null,
  };
}
