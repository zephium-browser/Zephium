<script lang="ts">
  import { untrack } from "svelte";
  import * as m from "$shared/i18n/messages";
  import { preferences } from "$domain/preferences";
  import Button from "$shared/ui/Button";
  import Field from "$shared/ui/Field";
  import SettingsRow from "$shared/ui/SettingsRow";

  let draft = $state(untrack(() => preferences.value("tabs.home")));
  let editing = $state(false);
  let invalid = $state(false);
  let saved = $derived(preferences.value("tabs.home"));
  $effect(() => {
    const value = preferences.value("tabs.home");
    if (!editing) draft = value;
  });
</script>

<SettingsRow
  settingId="general.home"
  title={m.pref_general_home()}
  description={m.pref_general_home_help()}
>
  <form
    onsubmit={(event) => {
      event.preventDefault();
      void preferences.set("tabs.home", draft.trim()).then(() => {
        invalid = preferences.saveFailed();
        if (!invalid) editing = false;
      });
    }}
  >
    <Field
      label={m.pref_general_home()}
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
