<script lang="ts">
  import { onDestroy } from "svelte";
  import { tabs } from "$domain/tabs";
  import { commands } from "$shared/ipc/bindings";
  import { retryImage } from "$session/newtab-background.svelte";
  import Button from "$shared/ui/Button";
  import * as m from "$shared/i18n/messages";
  import { preferences } from "$domain/preferences";
  import Select from "$shared/ui/Select";
  import Switch from "$shared/ui/Switch";
  import SettingsGroup from "$shared/ui/SettingsGroup";
  import SettingsRow from "$shared/ui/SettingsRow";

  let greeting = $derived(preferences.value("ui.newtab-greeting") === "true");
  let clock = $derived(preferences.value("ui.newtab-clock") === "true");

  let importing = $state(false);
  let resetting = $state(false);
  let failure = $state("");
  let live = true;
  onDestroy(() => {
    live = false;
  });
  let busy = $derived(importing || resetting || preferences.saving());
  let ownImage = $derived(preferences.value("ui.newtab-background") === "image");

  async function importImage() {
    const owner = tabs.profile()?.id;
    if (!owner || busy) return;
    importing = true;
    failure = "";
    try {
      const result = await commands.mediaImport(owner);
      if (!live || tabs.profile()?.id !== owner) return;
      if (result.status !== "ok") {
        failure = m.ntp_image_failed();
        return;
      }
      if (result.data.kind === "cancelled") return;
      if (result.data.kind !== "imported") {
        failure = m.ntp_image_failed();
        return;
      }
      const content = result.data.record.draft.content;
      if (content.kind !== "media" || content.asset.kind !== "image") {
        failure = m.ntp_image_failed();
        return;
      }
      await preferences.set("ui.newtab-image", content.asset.digest);
      if (preferences.saveFailed() || !live || tabs.profile()?.id !== owner) return;
      retryImage();
      await preferences.set("ui.newtab-background", "image");
    } catch {
      if (live) failure = m.ntp_image_failed();
    } finally {
      importing = false;
    }
  }
  function chooseBackground(value: string) {
    failure = "";
    if (value === "image" && !preferences.value("ui.newtab-image")) void importImage();
    else {
      retryImage();
      void preferences.set("ui.newtab-background", value);
    }
  }
  async function reset() {
    if (busy) return;
    resetting = true;
    failure = "";
    try {
      if (!(await preferences.resetNewTab())) failure = m.about_reset_failed();
      else retryImage();
    } finally {
      resetting = false;
    }
  }

  const set = (key: preferences.PreferenceKey, value: string) => void preferences.set(key, value);
</script>

<SettingsGroup title={m.ntp_background()}>
  <SettingsRow
    settingId="ntp.background"
    title={m.ntp_background()}
    description={m.ntp_background_help()}
  >
    <Select
      label={m.ntp_background()}
      labelHidden
      bind:value={() => preferences.value("ui.newtab-background"), chooseBackground}
      disabled={busy}
      options={[
        { value: "sky", label: m.ntp_sky() },
        { value: "image", label: m.ntp_own_image() },
      ]}
    />
  </SettingsRow>
  {#if ownImage}
    <SettingsRow title={m.ntp_own_image()}>
      <Button size="compact" disabled={busy} onclick={() => void importImage()}
        >{m.ntp_choose_image()}</Button
      >
    </SettingsRow>
    <SettingsRow settingId="ntp.dim" title={m.ntp_dim()} description={m.ntp_dim_help()}>
      <Switch
        label={m.ntp_dim()}
        labelHidden
        checked={preferences.value("ui.newtab-dim") === "true"}
        disabled={busy}
        onchange={(v) => set("ui.newtab-dim", String(v))}
      />
    </SettingsRow>
  {/if}
</SettingsGroup>

<SettingsGroup title={m.settings_newtab_content()}>
  <SettingsRow
    settingId="ntp.wordmark"
    title={m.ntp_wordmark()}
    description={m.ntp_wordmark_help()}
  >
    <Switch
      label={m.ntp_wordmark()}
      labelHidden
      checked={preferences.value("ui.newtab-wordmark") !== "false"}
      disabled={busy}
      onchange={(v) => set("ui.newtab-wordmark", String(v))}
    />
  </SettingsRow>
  <SettingsRow
    settingId="ntp.greeting"
    title={m.pref_ntp_greeting()}
    description={m.pref_ntp_greeting_help()}
    ><Switch
      label={m.pref_ntp_greeting()}
      labelHidden
      checked={greeting}
      disabled={busy}
      onchange={(v) => set("ui.newtab-greeting", String(v))}
    /></SettingsRow
  >
  <SettingsRow
    settingId="ntp.personalize"
    title={m.pref_ntp_personalize()}
    description={m.pref_ntp_personalize_help()}
    ><Switch
      label={m.pref_ntp_personalize()}
      labelHidden
      checked={preferences.value("ui.newtab-name") === "true"}
      disabled={busy || !greeting}
      onchange={(v) => set("ui.newtab-name", String(v))}
    /></SettingsRow
  >
  <SettingsRow
    settingId="ntp.clock"
    title={m.pref_ntp_clock()}
    description={m.pref_ntp_clock_help()}
    ><Switch
      label={m.pref_ntp_clock()}
      labelHidden
      checked={clock}
      disabled={busy}
      onchange={(v) => set("ui.newtab-clock", String(v))}
    /></SettingsRow
  >
  <SettingsRow
    settingId="ntp.clock-format"
    title={m.pref_ntp_clock_format()}
    description={m.pref_ntp_clock_format_help()}
    ><Select
      label={m.pref_ntp_clock_format()}
      labelHidden
      value={preferences.value("ui.newtab-clock-format")}
      disabled={busy || !clock}
      options={[
        { value: "system", label: m.pref_ntp_clock_format_option_0() },
        { value: "12h", label: m.pref_ntp_clock_format_option_1() },
        { value: "24h", label: m.pref_ntp_clock_format_option_2() },
      ]}
      onchange={(v) => set("ui.newtab-clock-format", v)}
    /></SettingsRow
  >
</SettingsGroup>

<SettingsGroup title={m.ntp_figures()}>
  <SettingsRow settingId="ntp.trackers" title={m.ntp_trackers_blocked()}>
    <Switch
      label={m.ntp_trackers_blocked()}
      labelHidden
      checked={preferences.value("ui.newtab-trackers") !== "false"}
      disabled={busy}
      onchange={(v) => set("ui.newtab-trackers", String(v))}
    />
  </SettingsRow>
  {#if preferences.enabled("time")}
    <SettingsRow settingId="ntp.focus" title={m.ntp_focused()}>
      <Switch
        label={m.ntp_focused()}
        labelHidden
        checked={preferences.value("ui.newtab-focus") !== "false"}
        disabled={busy}
        onchange={(v) => set("ui.newtab-focus", String(v))}
      />
    </SettingsRow>
  {/if}
  {#if preferences.enabled("tasks")}<SettingsRow
      settingId="ntp.tasks"
      title={m.pref_ntp_tasks()}
      description={m.pref_ntp_tasks_help()}
      ><Switch
        label={m.pref_ntp_tasks()}
        labelHidden
        checked={preferences.value("ui.newtab-tasks") === "true"}
        disabled={busy}
        onchange={(v) => set("ui.newtab-tasks", String(v))}
      /></SettingsRow
    >{/if}
</SettingsGroup>

<SettingsGroup title={m.ntp_reset()}>
  <SettingsRow title={m.ntp_reset()} description={m.ntp_reset_help()}>
    <Button size="compact" disabled={busy} onclick={() => void reset()}>{m.ntp_reset()}</Button>
  </SettingsRow>
</SettingsGroup>
{#if failure}<p class="failure" role="alert">{failure}</p>{/if}

<style>
  .failure {
    margin: 0;
    color: var(--color-danger);
    font-size: var(--text-label);
  }
</style>
