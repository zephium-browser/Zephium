/**
 * The person's keymap as native resolved it: every registry command, its
 * current keys and its defaults. Native owns validation and storage; this is
 * the mirror Keyboard settings draws and the chrome key handler matches.
 */
import { commands, type KeymapEntry, type KeymapOutcome } from "$shared/ipc/bindings";
import { events } from "$shared/ipc/native-events";
import { sameAccelerator } from "$shared/lib/accelerator";

let entries = $state.raw<KeymapEntry[]>([]);
let epoch = 0;
let stop: (() => void) | null = null;
let initializing: Promise<void> | null = null;

export const all = () => entries;

async function refresh(generation: number) {
  try {
    const next = await commands.keymapEntries();
    if (generation === epoch) entries = next;
  } catch {
    // The last known keymap stands; native retries on the next change.
  }
}

export function init(): Promise<void> {
  if (initializing) return initializing;
  const generation = ++epoch;
  initializing = (async () => {
    const unsubscribe = await events.keymapChanged.listen(() => void refresh(epoch));
    if (generation !== epoch) {
      unsubscribe();
      return;
    }
    stop = unsubscribe;
    await refresh(generation);
  })();
  return initializing;
}

export function dispose() {
  epoch += 1;
  stop?.();
  stop = null;
  initializing = null;
  entries = [];
}

/** The command a chrome key press names, if any. The launcher is global and
 *  Work keys belong to the pane, so neither is matched here. */
export function commandFor(event: KeyboardEvent, mac: boolean, pressed: string): string | null {
  if (event.isComposing || event.repeat) return null;
  let hit = entries.find(
    (entry) =>
      entry.group !== "global" &&
      entry.group !== "work" &&
      entry.accelerator !== null &&
      sameAccelerator(entry.accelerator, pressed, mac),
  );
  if (!hit) {
    const key =
      event.code === "Equal" && event.key === "+"
        ? pressed.replace("Shift+", "")
        : event.code === "NumpadAdd"
          ? pressed.replace("NumpadAdd", "Equal")
          : event.code === "NumpadSubtract"
            ? pressed.replace("NumpadSubtract", "Minus")
            : null;
    const id = event.code === "NumpadSubtract" ? "zoom.out" : "zoom.in";
    if (key !== null)
      hit = entries.find(
        (entry) =>
          entry.id === id &&
          entry.accelerator !== null &&
          sameAccelerator(entry.accelerator, `CmdOrCtrl+${id === "zoom.in" ? "=" : "-"}`, mac) &&
          sameAccelerator(entry.accelerator, key, mac),
      );
  }
  return hit?.id ?? null;
}

/** `replace` takes the keys from whichever command holds them, in the same
 *  native change. */
export async function bind(
  id: string,
  accelerator: string | null,
  replace = false,
): Promise<KeymapOutcome> {
  try {
    const outcome = await commands.keymapBind(id, accelerator, replace);
    if (outcome.kind === "applied") await refresh(epoch);
    return outcome;
  } catch {
    return { kind: "unavailable" };
  }
}

export async function reset(id: string | null): Promise<boolean> {
  try {
    const done = await commands.keymapReset(id);
    if (done) await refresh(epoch);
    return done;
  } catch {
    return false;
  }
}

/** While recording, native window-level key handlers stand aside so the keys
 *  reach the recorder. */
export async function record(active: boolean): Promise<void> {
  await commands.keymapRecord(active).catch(() => {});
}
