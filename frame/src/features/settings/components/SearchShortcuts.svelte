<script lang="ts">
  import * as m from "$shared/i18n/messages";
  import { preferences } from "$domain/preferences";
  import { favicons } from "$domain/favicons";
  import type { SearchShortcut } from "$shared/ipc/bindings";
  import SettingsGroup from "$shared/ui/SettingsGroup";
  import Switch from "$shared/ui/Switch";
  import Field from "$shared/ui/Field";
  import Button from "$shared/ui/Button";
  import IconButton from "$shared/ui/IconButton";
  import FavIcon from "$shared/ui/FavIcon";
  import EngineMark, { engineMarkUrl } from "$shared/ui/EngineMark";
  import { Cancel01Icon, Globe02Icon } from "@hugeicons/core-free-icons";
  import { SvelteSet } from "svelte/reactivity";
  import { customShortcut, shortcutKeywords } from "../lib/search-shortcuts";

  let invalid = $state(false);
  let adding = $state(false);
  let more = $state(false);
  let entries = $derived(preferences.searchShortcuts());
  // What is on comes first; the rest wait under one row, so the list is as
  // long as what is used. A row switched here stays where it is.
  const touched = new SvelteSet<string>();
  let on = $derived(entries.filter((entry) => entry.enabled || touched.has(entry.id)));
  let off = $derived(entries.filter((entry) => !entry.enabled && !touched.has(entry.id)));

  function save(next: SearchShortcut[] | "") {
    invalid = false;
    return preferences.set("search.shortcuts", next === "" ? "" : JSON.stringify(next));
  }

  async function rename(input: HTMLInputElement, entry: SearchShortcut) {
    const draft = input.value;
    const value = draft.trim();
    if (value === entry.keywords.join(" ")) return;
    const keywords = shortcutKeywords(value, entry.id, entries);
    if (!keywords) {
      invalid = true;
      input.value = entry.keywords.join(" ");
      return;
    }
    await save(entries.map((item) => (item.id === entry.id ? { ...item, keywords } : item)));
    // The input was edited directly; an unchanged projection won't reset it.
    if (!input.isConnected || input.value !== draft) return;
    const saved = preferences.searchShortcuts().find((item) => item.id === entry.id);
    if (saved) input.value = saved.keywords.join(" ");
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
    if (!preferences.saveFailed()) {
      form.reset();
      adding = false;
    }
  }
</script>

{#snippet row(entry: SearchShortcut)}
  {@const mark = favicons.forPage(entry.home_url)}
  <li class="shortcut" data-enabled={entry.enabled}>
    <span class="mark" aria-hidden="true"
      >{#if engineMarkUrl(entry.id)}<EngineMark engine={entry.id} size={18} />{:else}<FavIcon
          image={mark?.image ?? null}
          tone={mark?.tone}
          size={18}
          lit
          fallback={Globe02Icon}
        />{/if}</span
    >
    <span class="name">{entry.name}<span class="domain">{entry.domain}</span></span>
    <label class="keyword">
      <input
        aria-label={`${m.search_shortcut_keyword()}, ${entry.name}`}
        value={entry.keywords.join(" ")}
        maxlength={263}
        spellcheck="false"
        autocomplete="off"
        disabled={preferences.saving()}
        onkeydown={(event) => {
          if (event.key === "Enter") event.currentTarget.blur();
          else if (event.key === "Escape") {
            event.currentTarget.value = entry.keywords.join(" ");
            event.currentTarget.blur();
          }
        }}
        onblur={(event) => void rename(event.currentTarget, entry)}
      />
    </label>
    <Switch
      label={m.search_shortcut_enabled({ name: entry.name })}
      labelHidden
      checked={entry.enabled}
      disabled={preferences.saving()}
      onchange={(enabled) => {
        touched.add(entry.id);
        void save(entries.map((item) => (item.id === entry.id ? { ...item, enabled } : item)));
      }}
    />
    {#if entry.id.startsWith("custom:")}<span class="remove"
        ><IconButton
          icon={Cancel01Icon}
          label={`${m.search_shortcut_remove()}, ${entry.name}`}
          disabled={preferences.saving()}
          onclick={() => void save(entries.filter((item) => item.id !== entry.id))}
        /></span
      >{/if}
  </li>
{/snippet}

<SettingsGroup title={m.search_shortcuts_title()} description={m.search_shortcuts_help()}>
  <ul class="shortcuts" aria-label={m.search_shortcuts_title()}>
    {#each on as entry (entry.id)}{@render row(entry)}{/each}
    {#if more}{#each off as entry (entry.id)}{@render row(entry)}{/each}{/if}
  </ul>
  {#if invalid}<p class="invalid" role="alert">
      {m.search_shortcut_invalid({ query: "{query}" })}
    </p>{/if}
  {#if adding}
    <form class="add" onsubmit={add}>
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
        class="url"
        name="url"
        label={m.search_shortcut_url({ query: "{query}" })}
        placeholder={"https://example.com/search?q={query}"}
        maxlength={2048}
        required
        spellcheck={false}
        disabled={preferences.saving()}
      />
      <div class="actions">
        <Button variant="ghost" onclick={() => (adding = false)}>{m.action_cancel()}</Button>
        <Button type="submit" disabled={preferences.saving() || entries.length >= 64}
          >{m.action_add()}</Button
        >
      </div>
    </form>
  {/if}
  <div class="foot">
    {#if off.length}<Button variant="ghost" onclick={() => (more = !more)}
        >{more
          ? m.search_shortcuts_fewer()
          : m.search_shortcuts_more({ count: off.length })}</Button
      >{/if}
    {#if !adding}<Button variant="ghost" onclick={() => (adding = true)}
        >{m.search_shortcut_add()}</Button
      >{/if}
    <span class="spacer"></span>
    <Button variant="ghost" disabled={preferences.saving()} onclick={() => void save("")}
      >{m.search_shortcuts_reset()}</Button
    >
  </div>
</SettingsGroup>

<style>
  .shortcuts {
    margin: 0;
    padding: 0;
    list-style: none;
  }

  /* A row per place: its mark and name, the keyword it answers to, and
     whether it is on. */
  .shortcut {
    position: relative;
    display: grid;
    grid-template-columns: 18px minmax(0, 1fr) 132px auto;
    align-items: center;
    gap: 12px;
    min-block-size: 48px;
    padding: 6px 18px;
  }

  .shortcut:has(.remove) {
    grid-template-columns: 18px minmax(0, 1fr) 132px auto auto;
  }

  .shortcut + .shortcut::before {
    content: "";
    position: absolute;
    inset-inline: 48px 0;
    top: 0;
    height: 1px;
    background: var(--color-border);
  }

  .mark {
    display: grid;
    place-items: center;
  }

  .shortcut[data-enabled="false"] .mark {
    opacity: 0.55;
    filter: grayscale(1);
  }

  .name {
    display: flex;
    align-items: baseline;
    gap: 8px;
    min-inline-size: 0;
    overflow: hidden;
    color: var(--color-text);
    font-size: var(--text-body);
    font-weight: 500;
    white-space: nowrap;
  }

  .domain {
    overflow: hidden;
    color: var(--color-faint);
    font-size: var(--text-label);
    font-weight: 400;
    text-overflow: ellipsis;
  }

  /* The keywords it answers to, edited in place. */
  .keyword {
    display: flex;
    align-items: center;
    block-size: 28px;
    padding-inline: 10px 6px;
    border-radius: var(--radius-control);
    background: var(--color-fill);
    color: var(--color-muted);
    font-size: var(--text-label);
    font-variant-numeric: tabular-nums;
    transition: box-shadow var(--motion-instant) var(--ease-smooth);
  }

  .keyword:focus-within {
    box-shadow: var(--shadow-field-focus);
  }

  .keyword input {
    min-inline-size: 0;
    inline-size: 100%;
    padding: 0 0 0 1px;
    border: 0;
    outline: none;
    background: transparent;
    color: var(--color-text);
    font: inherit;
  }

  .invalid {
    margin: 0;
    padding: 4px 18px 8px;
    color: var(--color-danger);
    font-size: var(--text-label);
  }

  .add {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 10px 12px;
    padding: 14px 18px;
    border-block-start: 1px solid var(--color-border);
  }

  .add :global(.url) {
    grid-column: 1 / -1;
  }

  .actions {
    display: flex;
    grid-column: 1 / -1;
    justify-content: flex-end;
    gap: 8px;
  }

  .foot {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 8px 12px;
    border-block-start: 1px solid var(--color-border);
  }

  .spacer {
    flex: 1;
  }
</style>
