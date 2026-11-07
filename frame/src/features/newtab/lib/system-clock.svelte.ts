import { commands } from "$shared/ipc/bindings";

let hour24 = $state<boolean | null>(null);

/** Whether the system is set to 24-hour time; null until known or where the
 *  host has no such setting. */
export const systemHour24 = () => hour24;

export async function readSystemClock() {
  try {
    hour24 = await commands.systemClock24h();
  } catch {
    // Without an answer the locale's own hour cycle stands.
  }
}
