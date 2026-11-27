<script lang="ts">
  import { untrack } from "svelte";
  import * as m from "$shared/i18n/messages";
  import { tabs } from "$domain/tabs";
  import { DownloadSession, displayDirectory } from "$domain/downloads";
  import SettingsGroup from "$shared/ui/SettingsGroup";
  import SettingsRow from "$shared/ui/SettingsRow";
  import Button from "$shared/ui/Button";
  import Switch from "$shared/ui/Switch";
  let profile = $derived(tabs.profile()?.id ?? "unbound");
  let session = $state.raw(untrack(() => new DownloadSession(profile)));
  $effect(() => {
    const next = new DownloadSession(profile);
    session = next;
    void next.perform({ kind: "preferences" });
    return () => next.stop();
  });
</script>

<SettingsGroup
  title={m.section_downloads()}
  description={session.error === "unsupported"
    ? m.download_error_unsupported()
    : session.error
      ? m.download_error_unavailable()
      : undefined}
>
  <SettingsRow
    settingId="downloads.path"
    title={m.pref_downloads_path()}
    description={session.preferences?.directory
      ? displayDirectory(session.preferences.directory)
      : session.preferences || session.error
        ? m.download_default_directory()
        : m.download_settings_loading()}
  >
    <Button
      disabled={session.busy || !session.preferences || !session.supported}
      onclick={() => void session.perform({ kind: "choose_directory" })}
      >{m.settings_choose_folder()}</Button
    >
  </SettingsRow>
  <SettingsRow
    settingId="downloads.ask"
    title={m.pref_downloads_ask()}
    description={session.siteDownloadsRequireConfirmation
      ? m.download_windows_confirmation_help()
      : m.pref_downloads_ask_help()}
  >
    <Switch
      label={m.pref_downloads_ask()}
      labelHidden
      checked={session.siteDownloadsRequireConfirmation ||
        (session.preferences?.ask_destination ?? false)}
      disabled={session.busy ||
        !session.preferences ||
        !session.supported ||
        session.siteDownloadsRequireConfirmation}
      onchange={(enabled) => void session.perform({ kind: "set_ask_destination", enabled })}
    />
  </SettingsRow>
</SettingsGroup>
