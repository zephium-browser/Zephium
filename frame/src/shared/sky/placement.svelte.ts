/** Where the new tab's picture lies in the window: the page's left edge, top
 *  and height, and the picture's offset and height within it. The window's glass
 *  beside the page takes its light from the picture there. Kept after the new
 *  tab goes, so the light fades out where it stood. */
export type SkyPlacement = {
  left: number;
  top: number;
  height: number;
  y: number;
  h: number;
};

let placed = $state.raw<SkyPlacement | null>(null);

export const skyPlacement = () => placed;

export function placeSkyInWindow(next: SkyPlacement) {
  if (
    placed &&
    placed.left === next.left &&
    placed.top === next.top &&
    placed.height === next.height &&
    placed.y === next.y &&
    placed.h === next.h
  )
    return;
  placed = next;
}
