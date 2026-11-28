import {
  Airplane01Icon,
  Book02Icon,
  Briefcase01Icon,
  BulbIcon,
  Camera01Icon,
  CodeIcon,
  Coffee01Icon,
  Dumbbell01Icon,
  FavouriteIcon,
  GameController01Icon,
  Globe02Icon,
  Home01Icon,
  Leaf01Icon,
  Moon02Icon,
  Mortarboard01Icon,
  MusicNote01Icon,
  PaintBoardIcon,
  Rocket01Icon,
  ShoppingBag01Icon,
  StarIcon,
} from "@hugeicons/core-free-icons";

/** The curated symbols, in the order Rust stores their index. */
export const SPACE_SYMBOLS = [
  Globe02Icon,
  Briefcase01Icon,
  Home01Icon,
  CodeIcon,
  PaintBoardIcon,
  Book02Icon,
  Mortarboard01Icon,
  MusicNote01Icon,
  Camera01Icon,
  GameController01Icon,
  Coffee01Icon,
  Leaf01Icon,
  FavouriteIcon,
  StarIcon,
  Rocket01Icon,
  Airplane01Icon,
  ShoppingBag01Icon,
  Dumbbell01Icon,
  BulbIcon,
  Moon02Icon,
] as const;

/** Avatar hues, shared with profiles. */
export const SPACE_COLORS = 10;

/** One emoji as typed or pasted, or null when the text is not one. */
export function emojiOf(text: string): string | null {
  const trimmed = text.trim();
  if (!trimmed) return null;
  const segments = [...new Intl.Segmenter(undefined, { granularity: "grapheme" }).segment(trimmed)];
  const first = segments[0]?.segment ?? "";
  const pictographic = /\p{Extended_Pictographic}|⃣|\p{Regional_Indicator}/u.test(first);
  return pictographic && new TextEncoder().encode(first).length <= 32 ? first : null;
}
