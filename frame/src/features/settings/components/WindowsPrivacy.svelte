<script lang="ts">
  import { onMount } from "svelte";
  import { preferences } from "$domain/preferences";
  import { events } from "$shared/ipc/native-events";
  import { observe } from "$shared/lib/observe";
  import * as m from "$shared/i18n/messages";
  import { commands } from "$shared/ipc/bindings";
  import SettingsGroup from "$shared/ui/SettingsGroup";
  import SettingsRow from "$shared/ui/SettingsRow";
  import Button from "$shared/ui/Button";
  import Switch from "$shared/ui/Switch";

  let managed = $state(false);
  let launchUnavailable = $state(false);
  let checking = $state(true);
  let unavailable = $state(false);
  onMount(() => {
    const lifetime = new AbortController();
    const listening = events.windowsReputationManaged.listen(() => {
      managed = true;
    });
    void observe(commands.windowsReputationManaged(), 2000, lifetime.signal).then((result) => {
      if (lifetime.signal.aborted) return;
      if (result.state === "received") {
        if (result.value === null) launchUnavailable = true;
        else managed ||= result.value;
      } else unavailable = true;
      checking = false;
    });
    return () => {
      lifetime.abort();
      void listening.then((unlisten) => unlisten());
    };
  });

  let opening = $state(false);
  let failed = $state(false);
  async function openDiagnostics() {
    if (opening) return;
    opening = true;
    failed = false;
    try {
      failed = !(await commands.windowsDiagnosticsSettings());
    } catch {
      failed = true;
    } finally {
      opening = false;
    }
  }
</script>

<SettingsGroup title={m.privacy_windows_group()} description={m.privacy_windows_policy()}>
  <SettingsRow
    settingId="privacy.smartscreen"
    title={m.privacy_smartscreen()}
    description={managed
      ? m.privacy_smartscreen_managed()
      : launchUnavailable
        ? m.privacy_smartscreen_launch_unavailable()
        : unavailable
          ? m.privacy_smartscreen_unavailable()
          : m.privacy_smartscreen_help()}
  >
    <Switch
      label={m.privacy_smartscreen()}
      labelHidden
      checked={managed || launchUnavailable || preferences.value("privacy.smartscreen") === "true"}
      disabled={managed || launchUnavailable || checking || unavailable || preferences.saving()}
      onchange={(value) => void preferences.set("privacy.smartscreen", String(value))}
    />
  </SettingsRow>
  <SettingsRow
    title={m.privacy_windows_diagnostics()}
    description={m.privacy_windows_diagnostics_help()}
  >
    <Button disabled={opening} onclick={() => void openDiagnostics()}
      >{m.privacy_windows_diagnostics_open()}</Button
    >
  </SettingsRow>
  <SettingsRow
    title={m.privacy_microsoft_statement()}
    description={m.privacy_microsoft_statement_help()}
  >
    <Button
      variant="ghost"
      onclick={() =>
        void commands
          .browserOpenUrl("https://privacy.microsoft.com/privacystatement", true)
          .catch(() => {})}>{m.privacy_microsoft_statement_open()}</Button
    >
  </SettingsRow>
  {#if failed}<p class="feedback" role="status">{m.privacy_windows_diagnostics_failed()}</p>{/if}
</SettingsGroup>

<style>
  .feedback {
    margin: 12px 18px;
    color: var(--color-muted);
    font-size: var(--text-body);
  }
</style>
