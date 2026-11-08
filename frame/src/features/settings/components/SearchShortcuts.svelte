<script lang="ts">
  import * as m from "$shared/i18n/messages";
  import { preferences } from "$domain/preferences";
  import type { SearchShortcut } from "$shared/ipc/bindings";
  import SettingsGroup from "$shared/ui/SettingsGroup";
  import SettingsRow from "$shared/ui/SettingsRow";
  import Switch from "$shared/ui/Switch";
  import Field from "$shared/ui/Field";
  import Button from "$shared/ui/Button";
  import { customShortcut, shortcutKeywords } from "../lib/search-shortcuts";

  let invalid = $state(false);
  let entries = $derived(preferences.searchShortcuts());

  async function save(next: SearchShortcut[] | "") {
    invalid = false;
    await preferences.set("search.shortcuts", next === "" ? "" : JSON.stringify(next));
    invalid = preferences.saveFailed();
  }

  function edit(event: SubmitEvent, entry: SearchShortcut) {
    event.preventDefault();
    const form = event.currentTarget;
    if (!(form instanceof HTMLFormElement)) return;
    const keywords = shortcutKeywords(
      String(new FormData(form).get("keywords") ?? ""),
      entry.id,
      entries,
    );
    if (!keywords) {
      invalid = true;
      return;
    }
    void save(entries.map((item) => (item.id === entry.id ? { ...item, keywords } : item)));
  }

  async function add(event: SubmitEvent) {
    event.preventDefault();
    const form = event.currentTarget;
    if (!(form instanceof HTMLFormElement)) return;
    const data = new FormData(form);
    const entry = customShortcut(
      String(data.get("name") ?? ""),
      String(data.get("keyword") ?? ""),
      String(data.get("url") ?? ""),
      entries,
      `custom:${crypto.randomUUID()}`,
    );
    if (!entry) {
      invalid = true;
      return;
    }
    await save([...entries, entry]);
    if (!preferences.saveFailed()) form.reset();
  }
</script>

<SettingsGroup title={m.search_shortcuts_title()} description={m.search_shortcuts_help()}>
  {#each entries as entry (entry.id)}
    <SettingsRow title={entry.name}>
      <Switch
        label={m.search_shortcut_enabled({ name: entry.name })}
        labelHidden
        checked={entry.enabled}
        disabled={preferences.saving()}
        onchange={(enabled) =>
          void save(entries.map((item) => (item.id === entry.id ? { ...item, enabled } : item)))}
      />
      <form onsubmit={(event) => edit(event, entry)}>
        <Field
          name="keywords"
          label={m.search_shortcut_keyword()}
          value={entry.keywords.join(" ")}
          maxlength={263}
          spellcheck={false}
          disabled={preferences.saving()}
        />
        <Button type="submit" size="compact" disabled={preferences.saving()}
          >{m.search_save()}</Button
        >
      </form>
      {#if entry.id.startsWith("custom:")}
        <Button
          size="compact"
          disabled={preferences.saving()}
          onclick={() => void save(entries.filter((item) => item.id !== entry.id))}
          >{m.search_shortcut_remove()}</Button
        >
      {/if}
    </SettingsRow>
  {/each}
  <form onsubmit={add}>
    <Field
      name="name"
      label={m.search_shortcut_name()}
      maxlength={80}
      required
      disabled={preferences.saving()}
    />
    <Field
      name="keyword"
      label={m.search_shortcut_keyword()}
      maxlength={32}
      required
      spellcheck={false}
      disabled={preferences.saving()}
    />
    <Field
      name="url"
      label={m.search_shortcut_url({ query: "{query}" })}
      maxlength={2048}
      required
      spellcheck={false}
      disabled={preferences.saving()}
    />
    <Button type="submit" disabled={preferences.saving() || entries.length >= 64}
      >{m.search_shortcut_add()}</Button
    >
  </form>
  {#if invalid}<p role="alert">{m.search_shortcut_invalid({ query: "{query}" })}</p>{/if}
  <Button variant="secondary" disabled={preferences.saving()} onclick={() => void save("")}
    >{m.search_shortcuts_reset()}</Button
  >
</SettingsGroup>
