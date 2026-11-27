/**
 * Words typed but not yet sent, kept in the chrome webview's own storage so
 * they outlive a relaunch. A draft is a convenience, not a record: it is
 * written as the person types, read once when its owner is made, and never a
 * reason to hold up closing. Anything that goes wrong here is quiet.
 */
const ROOT = "zephium.draft.";
const MAX_CHARS = 64 * 1024;
const LIFETIME_MS = 90 * 24 * 60 * 60 * 1000;

const keyOf = (kind: string, scope: readonly string[]) => [ROOT + kind, ...scope].join(".");

function storage(): Storage | null {
  try {
    return globalThis.localStorage ?? null;
  } catch {
    return null;
  }
}

/** What was kept for `scope`, or null when nothing usable was. */
export function readDraft(kind: string, scope: readonly string[]): string | null {
  const store = storage();
  if (!store) return null;
  const key = keyOf(kind, scope);
  try {
    const raw = store.getItem(key);
    if (raw === null) return null;
    const kept: unknown = JSON.parse(raw);
    if (
      typeof kept === "object" &&
      kept !== null &&
      "text" in kept &&
      "at" in kept &&
      "v" in kept &&
      kept.v === 1 &&
      typeof kept.text === "string" &&
      typeof kept.at === "number" &&
      Date.now() - kept.at < LIFETIME_MS &&
      kept.text.trim()
    )
      return kept.text;
    store.removeItem(key);
  } catch {
    // Unreadable storage: there is no draft.
  }
  return null;
}

/** Keeps `text`, or lets the draft go when there is nothing worth keeping. */
export function writeDraft(kind: string, scope: readonly string[], text: string): void {
  const store = storage();
  if (!store) return;
  const key = keyOf(kind, scope);
  try {
    if (!text.trim() || text.length > MAX_CHARS) store.removeItem(key);
    else store.setItem(key, JSON.stringify({ v: 1, at: Date.now(), text }));
  } catch {
    // Full or blocked storage: the words stay in this document only.
  }
}

const forgetting = new Set<(profile: string, space?: string) => void>();

/** Tells `owner` whenever drafts are let go, so a live one can empty itself. */
export function onDraftsForgotten(owner: (profile: string, space?: string) => void): void {
  forgetting.add(owner);
}

/** Lets go of every draft of a profile, or of one of its spaces, once it is deleted. */
export function forgetDrafts(profile: string, space?: string): void {
  for (const owner of forgetting) owner(profile, space);
  const store = storage();
  if (!store) return;
  try {
    const doomed: string[] = [];
    for (let index = 0; index < store.length; index += 1) {
      const key = store.key(index);
      if (!key?.startsWith(ROOT)) continue;
      const [, owner, within] = key.slice(ROOT.length).split(".");
      if (owner === profile && (space === undefined || within === space)) doomed.push(key);
    }
    for (const key of doomed) store.removeItem(key);
  } catch {
    // What cannot be listed cannot be dropped; it expires on its own.
  }
}
