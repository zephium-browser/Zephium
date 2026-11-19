<script lang="ts">
  import { onMount } from "svelte";
  import * as m from "$shared/i18n/messages";
  import { commands } from "$shared/ipc/bindings";
  import type { DoubleTap, LauncherTrigger, Rejection } from "$shared/ipc/bindings";
  import { IS_MAC } from "$shared/platform";
  import { acceleratorFrom, acceleratorKeys, heldModifiers } from "$shared/lib/accelerator";
  import SettingsGroup from "$shared/ui/SettingsGroup";
  import SettingsRow from "$shared/ui/SettingsRow";
  import Select from "$shared/ui/Select";
  import Button from "$shared/ui/Button";
  import ShortcutField from "./ShortcutField.svelte";

  let trigger = $state<LauncherTrigger | null>(null);
  let recording = $state(false);
  let applying = $state(false);
  let disposed = false;
  let held = $state<string[]>([]);
  let rejection = $state<Rejection | null>(null);
  let failed = $state(false);

  let keys = $derived(trigger ? acceleratorKeys(trigger.shortcut, IS_MAC) : []);
  let custom = $derived(!!trigger && trigger.shortcut !== trigger.default_shortcut);
  let waiting = $derived(!!trigger && trigger.double_tap !== "off" && !trigger.accessibility);

  const reasons: Record<Rejection, () => string> = {
    invalid: m.launcher_shortcut_invalid,
    needs_modifier: IS_MAC
      ? m.launcher_shortcut_needs_modifier
      : m.launcher_shortcut_needs_modifier_windows,
    system: m.launcher_shortcut_system,
    app_command: m.launcher_shortcut_app_command,
    unavailable: m.launcher_shortcut_unavailable,
    unsupported: m.launcher_shortcut_unsupported,
  };

  let status = $derived.by(() => {
    if (rejection) return reasons[rejection]();
    if (recording) return m.launcher_shortcut_recording();
    if (failed) return m.panel_action_failed();
    if (trigger && !trigger.registered) return m.launcher_shortcut_unregistered();
    return m.launcher_shortcut_ready();
  });

  async function refresh() {
    try {
      trigger = await commands.launcherTrigger();
    } catch {
      failed = true;
    }
  }

  async function begin() {
    if (!trigger?.editable || applying) return;
    rejection = null;
    failed = false;
    recording = true;
    held = [];
    await commands.launcherRecordShortcut(true).catch(() => {});
  }

  async function end() {
    if (!recording) return;
    recording = false;
    rejection = null;
    held = [];
    await commands.launcherRecordShortcut(false).catch(() => {});
    await refresh();
  }

  async function apply(accelerator: string) {
    if (applying) return;
    const listening = recording;
    applying = true;
    held = [];
    try {
      const change = await commands.launcherSetShortcut(accelerator);
      if (disposed) return;
      if (!change) {
        failed = true;
        await end();
        return;
      }
      trigger = change.trigger;
      if (change.type === "applied") {
        recording = false;
        rejection = null;
      } else if (!listening || recording) {
        rejection = change.reason;
        // A refusal can take native out of recording mode. Resume only
        // while this recorder still owns the capture.
        if (recording) await commands.launcherRecordShortcut(true).catch(() => {});
      }
    } catch {
      if (!disposed) {
        failed = true;
        await end();
      }
    } finally {
      applying = false;
    }
  }

  function keydown(event: KeyboardEvent) {
    if (!recording) return;
    event.preventDefault();
    event.stopPropagation();
    if (event.key === "Escape" && !event.metaKey && !event.ctrlKey && !event.altKey) {
      void end();
      return;
    }
    if (applying) return;
    const accelerator = acceleratorFrom(event, IS_MAC);
    if (accelerator) {
      rejection = null;
      void apply(accelerator);
    } else held = heldModifiers(event, IS_MAC);
  }

  function keyup(event: KeyboardEvent) {
    if (recording && !applying) held = heldModifiers(event, IS_MAC);
  }

  async function setDoubleTap(mode: DoubleTap) {
    try {
      trigger = (await commands.launcherSetDoubleTap(mode)) ?? trigger;
    } catch {
      failed = true;
    }
  }

  onMount(() => {
    void refresh();
    return () => {
      disposed = true;
      if (recording) void commands.launcherRecordShortcut(false).catch(() => {});
    };
  });
  // Permission is granted in System Settings, which tells nobody; look
  // again while this page is waiting for it, and only then.
  $effect(() => {
    if (!waiting) return;
    const poll = setInterval(() => {
      if (document.visibilityState === "visible") void refresh();
    }, 1500);
    return () => clearInterval(poll);
  });
</script>

<svelte:window onkeydown={keydown} onkeyup={keyup} onblur={() => void end()} />

<SettingsGroup title={m.launcher_settings()}>
  <SettingsRow title={m.launcher_shortcut()} description={status}>
    <div class="shortcut">
      {#if custom && !recording}<Button
          variant="ghost"
          size="compact"
          onclick={() => trigger && void apply(trigger.default_shortcut)}
          >{m.launcher_shortcut_reset()}</Button
        >{/if}
      <ShortcutField
        {keys}
        {held}
        {recording}
        prompt={m.launcher_shortcut_press()}
        warn={!!trigger && !trigger.registered}
        disabled={!trigger?.editable}
        label={recording ? m.launcher_shortcut_recording() : m.launcher_shortcut_record()}
        onclick={() => (recording ? void end() : void begin())}
      />
    </div>
  </SettingsRow>
  {#if trigger?.double_tap_supported}
    <SettingsRow
      title={m.launcher_double_tap()}
      description={waiting ? m.launcher_double_tap_permission() : m.launcher_double_tap_desc()}
    >
      <div class="shortcut">
        {#if waiting}<Button
            variant="secondary"
            size="compact"
            onclick={() => void commands.launcherOpenAccessibility()}
            >{m.launcher_open_accessibility()}</Button
          >{/if}
        <Select
          label={m.launcher_double_tap()}
          labelHidden
          value={trigger.double_tap}
          options={[
            { value: "off", label: m.launcher_double_tap_off() },
            { value: "command", label: m.launcher_double_tap_command() },
            { value: "option", label: m.launcher_double_tap_option() },
          ]}
          onchange={(value) => void setDoubleTap(value as DoubleTap)}
        />
      </div>
    </SettingsRow>
  {/if}
</SettingsGroup>

<style>
  .shortcut {
    display: flex;
    align-items: center;
    gap: 8px;
    flex: none;
  }
</style>
