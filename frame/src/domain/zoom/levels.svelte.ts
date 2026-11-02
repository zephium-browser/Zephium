import { commands } from "$shared/ipc/bindings";
import type { ZoomCall, ZoomLevelView } from "$shared/ipc/bindings";
import { observe } from "$shared/lib/observe";

const TIMEOUT = 8000;

/** The sites a profile has zoomed, as Rust keeps them. A change shows only
 *  once Rust answers with the list it now holds. */
export class ZoomLevelsSession {
  levels = $state.raw<ZoomLevelView[]>([]);
  loaded = $state(false);
  unavailable = $state(false);
  busy = $state(false);
  private lifetime = new AbortController();

  constructor(readonly profile: string) {}

  load() {
    return this.run({ kind: "levels" });
  }

  /** The site shows at the default zoom again. */
  forget(site: string) {
    return this.run({ kind: "forget", site });
  }

  forgetAll() {
    return this.run({ kind: "forget_all" });
  }

  dispose() {
    this.lifetime.abort();
  }

  private async run(call: ZoomCall) {
    if (this.busy) return false;
    this.busy = true;
    const answer = await observe(
      Promise.resolve().then(() => commands.zoomCall(this.profile, call)),
      TIMEOUT,
      this.lifetime.signal,
    );
    if (answer.state === "aborted") return false;
    this.busy = false;
    if (answer.state !== "received" || answer.value.kind !== "levels") {
      this.unavailable = true;
      return false;
    }
    this.unavailable = false;
    this.loaded = true;
    this.levels = answer.value.levels;
    return true;
  }
}
