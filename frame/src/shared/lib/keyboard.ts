/** A key still belongs to the input method, including WebKit's legacy marker. */
export function isComposing(event: Pick<KeyboardEvent, "isComposing" | "keyCode">): boolean {
  return event.isComposing || event.keyCode === 229;
}
