import {
  commands,
  type VoiceSessionPreferencesV1,
  type VoiceSessionSettingsV1,
} from "$shared/ipc/bindings";
import { observe } from "$shared/lib/observe";

const TIMEOUT = 9000;

/**
 * Settings → Voice: what Rust stores for talking to the assistant and the
 * voices the service offers. A change shows once Rust has stored it.
 */
export class TalkSettings {
  settings = $state.raw<VoiceSessionSettingsV1 | null>(null);
  unavailable = $state(false);
  /** The last change was refused or did not answer; the stored choice stands. */
  refused = $state(false);
  /** Counts refusals, so a control that moved on its own can be put back. */
  refusals = $state(0);
  busy = $state(false);
  private sequence = 0;
  private lifetime = new AbortController();

  private async adopt(request: () => Promise<VoiceSessionSettingsV1 | null>, change: boolean) {
    const sequence = ++this.sequence;
    const response = await observe(Promise.resolve().then(request), TIMEOUT, this.lifetime.signal);
    if (sequence !== this.sequence || response.state === "aborted") return;
    if (response.state !== "received" || !response.value) {
      if (change) {
        this.refused = true;
        this.refusals += 1;
      } else this.unavailable = true;
      return;
    }
    this.unavailable = false;
    this.refused = false;
    this.settings = response.value;
  }

  read() {
    return this.adopt(() => commands.voiceSessionSettings(), false);
  }

  async change(patch: Partial<VoiceSessionPreferencesV1>) {
    const current = this.settings?.preferences;
    if (!current) return;
    this.busy = true;
    await this.adopt(() => commands.voiceSessionSetSettings({ ...current, ...patch }), true);
    this.busy = false;
  }

  dispose() {
    this.lifetime.abort();
  }
}
