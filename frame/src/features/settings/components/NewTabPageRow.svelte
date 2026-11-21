<script lang="ts">
  import { untrack } from "svelte";
  import * as m from "$shared/i18n/messages";
  import { preferences } from "$domain/preferences";
  import Button from "$shared/ui/Button";
  import Field from "$shared/ui/Field";
  import SettingsRow from "$shared/ui/SettingsRow";

  let draft = $state(untrack(() => preferences.value("tabs.new-page")));
  let editing = $state(false);
  let invalid = $state(false);
  let saved = $derived(preferences.value("tabs.new-page"));
  $effect(() => {
    const value = preferences.value("tabs.new-page");
    if (!editing) draft = value;
  });

  async function save(event: SubmitEvent) {
    event.preventDefault();
    const submitted = draft;
    await preferences.set("tabs.new-page", submitted.trim());
    if (draft !== submitted) return;
    invalid = preferences.saveFailed();
    if (!invalid) editing = false;
  }
</script>

<SettingsRow settingId="ntp.page" title={m.ntp_page()} description={m.ntp_page_help()}>
  <form onsubmit={save}>
    <Field
      label={m.ntp_page()}
      labelHidden
      value={draft}
      autocomplete="off"
      inputmode="url"
      spellcheck={false}
      maxlength={2048}
      placeholder="https://example.com"
      error={invalid ? m.field_invalid_url() : undefined}
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

<style>
  form {
    display: flex;
    align-items: start;
    gap: 8px;
    min-inline-size: 0;
  }
</style>
