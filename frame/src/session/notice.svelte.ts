/** One brief confirmation, such as a copied link. A newer notice replaces the
 *  one showing; nothing queues. The kind picks the glyph that stands for it,
 *  which is all a collapsed sidebar shows. */
export type NoticeKind = "link" | "download" | "focus" | "data";
type Notice = { id: number; text: string; kind: NoticeKind };

let current = $state.raw<Notice | null>(null);
let next = 0;

export const notice = () => current;

export function show(text: string, kind: NoticeKind = "link") {
  next += 1;
  current = { id: next, text, kind };
}

/** Clears only the notice that asked, so a newer one is never cut short. */
export function dismiss(id: number) {
  if (current?.id === id) current = null;
}
