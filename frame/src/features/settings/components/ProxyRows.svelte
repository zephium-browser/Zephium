<script lang="ts">
  import { untrack } from "svelte";
  import * as m from "$shared/i18n/messages";
  import { preferences } from "$domain/preferences";
  import { IS_WINDOWS } from "$shared/platform";
  import Button from "$shared/ui/Button";
  import Field from "$shared/ui/Field";
  import Select from "$shared/ui/Select";
  import SettingsRow from "$shared/ui/SettingsRow";

  let mode = $derived(preferences.value("network.proxy"));
  let saved = $derived(preferences.value("network.proxy-address"));
  let draft = $state(untrack(() => preferences.value("network.proxy-address")));
  let editing = $state(false);
  let invalid = $state(false);
  $effect(() => {
    const value = preferences.value("network.proxy-address");
    if (!editing) draft = value;
  });
</script>

<SettingsRow
  settingId="network.proxy"
  title={m.pref_network_proxy()}
  description={IS_WINDOWS ? m.pref_network_proxy_help_windows() : m.pref_network_proxy_help_mac()}
>
  <Select
    label={m.pref_network_proxy()}
    labelHidden
    value={mode}
    disabled={preferences.saving()}
    options={[
      { value: "system", label: m.network_proxy_system() },
      { value: "http", label: m.network_proxy_http() },
      { value: "socks5", label: m.network_proxy_socks5() },
    ]}
    onchange={(value) => void preferences.set("network.proxy", value)}
  />
</SettingsRow>
{#if mode !== "system"}
  <SettingsRow
    settingId="network.proxy-address"
    title={m.pref_network_proxy_address()}
    description={m.pref_network_proxy_address_help()}
  >
    <form
      onsubmit={(event) => {
        event.preventDefault();
        void preferences.set("network.proxy-address", draft.trim()).then(() => {
          invalid = preferences.saveFailed();
          if (!invalid) editing = false;
        });
      }}
    >
      <Field
        label={m.pref_network_proxy_address()}
        labelHidden
        value={draft}
        autocomplete="off"
        spellcheck={false}
        maxlength={300}
        placeholder="proxy.example.com:3128"
        error={invalid ? m.network_proxy_address_invalid() : undefined}
        oninput={(event) => {
          editing = true;
          invalid = false;
          draft = event.currentTarget.value;
        }}
      />
      <Button type="submit" disabled={preferences.saving() || draft.trim() === saved}>
        {m.search_save()}
      </Button>
    </form>
  </SettingsRow>
{/if}

<style>
  form {
    display: flex;
    align-items: start;
    gap: 8px;
    min-inline-size: 0;
  }
</style>
