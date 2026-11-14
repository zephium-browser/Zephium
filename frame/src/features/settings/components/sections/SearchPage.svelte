<script lang="ts">
  import * as m from "$shared/i18n/messages";
  import { preferences } from "$domain/preferences";
  import SettingsGroup from "$shared/ui/SettingsGroup";
  import SettingsRow from "$shared/ui/SettingsRow";
  import Select from "$shared/ui/Select";
  import EngineMark from "$shared/ui/EngineMark";
  import Field from "$shared/ui/Field";
  import Button from "$shared/ui/Button";
  import Switch from "$shared/ui/Switch";
  import { untrack } from "svelte";
  import SearchShortcuts from "../SearchShortcuts.svelte";
  import PreferenceSwitch from "../PreferenceSwitch.svelte";

  let customUrl = $state(untrack(() => preferences.value("search.custom-url")));
  let editing = $state(false);
  $effect(() => {
    const value = preferences.value("search.custom-url");
    if (!editing) customUrl = value;
  });

  /** Feedback only, mirroring `zephium_core::search::valid_template`. Native
   *  remains the authority and rejects anything this misses. */
  let invalid = $derived.by(() => {
    const value = customUrl.trim();
    if (!value) return false;
    if (value.length > 2048 || value.split("{searchTerms}").length !== 2) return true;
    try {
      const url = new URL(value.replace("{searchTerms}", "zephium-query"));
      return (
        url.protocol !== "https:" ||
        !url.search.includes("zephium-query") ||
        url.hostname.includes("zephium-query")
      );
    } catch {
      return true;
    }
  });
  let saved = $derived(preferences.value("search.custom-url"));
  let engine = $derived(preferences.value("search.engine"));
</script>

<SettingsGroup title={m.section_search()} description={m.search_local_description()}>
  <SettingsRow
    settingId="search.provider"
    title={m.search_engine_label()}
    description={m.search_engine_description()}
  >
    <Select
      label={m.search_engine_label()}
      labelHidden
      value={engine}
      disabled={preferences.saving()}
      options={[
        { value: "google", label: "Google" },
        { value: "duckduckgo", label: "DuckDuckGo" },
        { value: "bing", label: "Bing" },
        { value: "brave", label: "Brave Search" },
        { value: "custom", label: m.search_custom(), disabled: !saved },
      ]}
      onchange={(value) => void preferences.set("search.engine", value)}
    >
      {#snippet lead(value)}<EngineMark engine={value} />{/snippet}
    </Select>
  </SettingsRow>

  <SettingsRow
    settingId="search.suggestions"
    title={m.search_suggestions_label()}
    description={engine !== "custom"
      ? m.search_suggestions_description()
      : m.search_suggestions_unavailable()}
  >
    <Switch
      label={m.search_suggestions_label()}
      labelHidden
      checked={preferences.value("search.suggestions") === "true"}
      disabled={preferences.saving()}
      onchange={(value) => void preferences.set("search.suggestions", String(value))}
    />
  </SettingsRow>

  <PreferenceSwitch id="search.history" preference="search.history" />
  <SettingsRow
    settingId="search.engines"
    title={m.search_custom()}
    description={m.search_custom_description()}
  >
    <form
      onsubmit={(event) => {
        event.preventDefault();
        if (invalid) return;
        void preferences.set("search.custom-url", customUrl).then(() => {
          if (!preferences.saveFailed()) editing = false;
        });
      }}
    >
      <Field
        label={m.search_custom()}
        labelHidden
        value={customUrl}
        maxlength={2048}
        spellcheck={false}
        placeholder={"https://example.com/search?q={searchTerms}"}
        error={invalid ? m.search_custom_invalid() : undefined}
        hint={invalid || saved ? undefined : m.search_custom_hint()}
        oninput={(event) => {
          editing = true;
          customUrl = event.currentTarget.value;
        }}
      />
      <Button type="submit" disabled={preferences.saving() || invalid}>{m.search_save()}</Button>
    </form>
  </SettingsRow>
</SettingsGroup>

<SearchShortcuts />

<style>
  form {
    display: flex;
    gap: 8px;
    align-items: start;
    min-inline-size: 0;
  }

  form :global(.ui-field) {
    inline-size: 260px;
  }
</style>
