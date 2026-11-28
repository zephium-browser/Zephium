import type { SpaceLookInputV1 } from "$shared/ipc/bindings";

export { SPACE_COLORS, SPACE_SYMBOLS, emojiOf } from "$shared/lib/space-symbols";

/** A new space wears nothing: no icon, no hue, the window's own theme. */
export const FRESH_LOOK: SpaceLookInputV1 = { icon: null, color: 0, theme: null };
