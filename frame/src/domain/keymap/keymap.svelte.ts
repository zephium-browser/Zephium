/**
 * The person's keymap as native resolved it: every registry command, its
 * current keys and its defaults. Native owns validation and storage; this is
 * the mirror Keyboard settings draws and the chrome key handler matches.
 */
import { commands, type KeymapEntry, type KeymapOutcome } from "$shared/ipc/bindings";
import { events } from "$shared/ipc/native-events";
import { acceleratorFrom, sameAccelerator } from "$shared/lib/accelerator";

let entries = $state.raw<KeymapEntry[]>([]);
let epoch = 0;
let refreshId = 0;
let stop: (() => void) | null = null;
let initializing: Promise<void> | null = null;

export const all = () => entries;

async function refresh(generation: number) {
  const id = ++refreshId;
  try {
    const next = await commands.keymapEntries();
    if (generation === epoch && id === refreshId) entries = next;
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
export function commandFor(
  event: KeyboardEvent,
  mac: boolean,
  pressed: string,
  windows = false,
): string | null {
  if (event.isComposing || event.repeat) return null;
  const exact = (accelerator: string) =>
    entries.find(
      (entry) =>
        entry.id !== "launcher.toggle" &&
        entry.group !== "work" &&
        entry.accelerator !== null &&
        sameAccelerator(entry.accelerator, accelerator, mac),
    );
  let hit = exact(pressed);
  if (
    !hit &&
    windows &&
    (/^Digit\d$/u.test(event.code) || (event.code === "Equal" && event.key === "+"))
  ) {
    const physical = acceleratorFrom(event, mac);
    if (physical !== null) hit = exact(physical);
  }
  if (!hit) {
    const key =
      event.code === "NumpadAdd"
        ? pressed.replace("NumpadAdd", "Equal")
        : event.code === "NumpadSubtract"
          ? pressed.replace("NumpadSubtract", "Minus")
          : event.key === "+" && (windows || event.code === "Equal")
            ? pressed.replace(/(?:Shift\+)?(?:\+|Equal)$/u, "Equal")
            : null;
    const id = event.code === "NumpadSubtract" ? "zoom.out" : "zoom.in";
    if (key !== null) {
      const fallback = `CmdOrCtrl+${id === "zoom.in" ? "=" : "-"}`;
      hit = entries.find((entry) => {
        if (entry.id !== id || (windows && entry.customized)) return false;
        const original = windows ? entry.default_accelerator : entry.accelerator;
        return (
          original !== null &&
          sameAccelerator(original, fallback, mac) &&
          sameAccelerator(original, key, mac)
        );
      });
    }
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
