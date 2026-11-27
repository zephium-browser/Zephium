const lowercase = (value: string) => value.toLocaleLowerCase();
let titleSegments: Intl.Segmenter | undefined;

/** The span of `title` the query accounts for, or null when it matches nowhere. */
export function matchRange(
  title: string,
  query: string,
  fold: (value: string) => string = lowercase,
): [number, number] | null {
  const needle = fold(query.trim());
  if (!needle) return null;
  const folded = fold(title);
  const at = folded.indexOf(needle);
  if (at < 0) return null;
  const end = at + needle.length;
  if (folded.length === title.length) return [at, end];

  // Lowercasing can expand one character (İ → i + combining dot). Offsets
  // in that string must be mapped back before slicing the original title.
  titleSegments ??= new Intl.Segmenter(undefined, { granularity: "grapheme" });
  let offset = 0;
  let start: number | undefined;
  for (const { segment, index } of titleSegments.segment(title)) {
    const next = offset + fold(segment).length;
    if (start === undefined && next > at) start = index;
    if (next >= end) return [start ?? index, index + segment.length];
    offset = next;
  }
  return null;
}
