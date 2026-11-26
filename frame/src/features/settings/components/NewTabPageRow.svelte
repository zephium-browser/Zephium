<script lang="ts">
  import { untrack } from "svelte";
  import * as m from "$shared/i18n/messages";
  import { preferences } from "$domain/preferences";
  import Button from "$shared/ui/Button";
  import Field from "$shared/ui/Field";

  let { resetting = false }: { resetting?: boolean } = $props();

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
    if (resetting || preferences.saving()) return;
    const submitted = draft;
    await preferences.set("tabs.new-page", submitted.trim());
    if (draft !== submitted) return;
    invalid = preferences.saveFailed();
    if (!invalid) editing = false;
  }
</script>

<div class="destination" data-setting="ntp.page">
  <div class="copy">
    <h3>{m.ntp_page()}</h3>
    <p>{m.ntp_page_help()}</p>
  </div>
  <form onsubmit={save}>
    <Field
      label={m.ntp_page()}
      labelHidden
      value={draft}
      disabled={resetting}
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
    <Button type="submit" disabled={resetting || preferences.saving() || draft.trim() === saved}>
      {m.search_save()}
    </Button>
  </form>
</div>

<style>
  .destination {
    display: grid;
    gap: 12px;
    min-inline-size: 0;
    padding: 14px 18px;
  }

  h3 {
    margin: 0;
    color: var(--color-text);
    font-size: var(--text-page-title);
    font-weight: 500;
    line-height: 19px;
    letter-spacing: -0.008em;
  }

  p {
    margin: 2px 0 0;
    color: var(--color-muted);
    font-size: var(--text-label);
    line-height: 1.5;
  }

  form {
    display: flex;
    align-items: start;
    gap: 8px;
    min-inline-size: 0;
  }

  form :global(.ui-field) {
    flex: 1;
  }
</style>
