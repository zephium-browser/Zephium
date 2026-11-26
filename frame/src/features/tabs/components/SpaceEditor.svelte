<script lang="ts">
  import * as m from "$shared/i18n/messages";
  import { Tick02Icon, UnavailableIcon } from "@hugeicons/core-free-icons";
  import type { SpaceIconView, SpaceThemeView, SpaceView } from "$shared/ipc/bindings";
  import { tabs } from "$domain/tabs";
  import { palette, theme as appearance } from "$domain/appearance";
  import { preferences } from "$domain/preferences";
  import Button from "$shared/ui/Button";
  import Icon from "$shared/ui/Icon";
  import { onMount, untrack } from "svelte";
  import "./space-hue.css";
  import { SPACE_COLORS, SPACE_SYMBOLS, emojiOf } from "../lib/space-look";

  let {
    space,
    fresh = false,
    deletable,
    onclose,
  }: {
    space: SpaceView;
    /** Made a moment ago: its name is selected, ready to be typed over. */
    fresh?: boolean;
    deletable: boolean;
    /** `done` when the person finished here rather than stepping back. */
    onclose: (done: boolean) => void;
  } = $props();

  // Every change applies as it is made. The editor keeps what it opened on,
  // so Escape can put the space back exactly as it was.
  const id = untrack(() => space.id);
  const original = untrack(() => ({
    name: space.name,
    icon: space.icon,
    color: space.color,
    theme: space.theme,
  }));
  let name = $state(original.name);
  let icon = $state<SpaceIconView | null>(original.icon);
  let color = $state(original.color);
  let chosen = $state<SpaceThemeView | null>(original.theme);
  let emoji = $state(original.icon?.kind === "emoji" ? original.icon.text : "");
  let failed = $state(false);
  let confirming = $state(false);
  let deleting = $state(false);
  let field = $state<HTMLInputElement>();
  let named = $state(original.name);
  let dirty = false;
  let sending = false;
  let again = false;
  let typing: ReturnType<typeof setTimeout> | undefined;

  async function commit() {
    clearTimeout(typing);
    typing = undefined;
    if (sending) {
      again = true;
      return;
    }
    sending = true;
    do {
      again = false;
      named = name.trim() || named;
      const result = await tabs.updateSpace(id, named, { icon, color, theme: chosen });
      failed = result.outcome === "failed" || result.outcome === "rejected";
    } while (again);
    sending = false;
  }

  function change(apply: () => void) {
    apply();
    dirty = true;
    void commit();
  }

  function typeName(text: string) {
    name = text;
    dirty = true;
    clearTimeout(typing);
    // A name lands once typing pauses, so every key is not a write.
    typing = setTimeout(() => void commit(), 450);
  }

  function typeEmoji(text: string) {
    emoji = text;
    const found = emojiOf(text);
    if (found) change(() => (icon = { kind: "emoji", text: found }));
  }

  function done(event?: SubmitEvent) {
    event?.preventDefault();
    if (typing !== undefined) void commit();
    onclose(true);
  }

  function restore() {
    if (dirty) {
      name = original.name;
      icon = original.icon;
      color = original.color;
      chosen = original.theme;
      void commit();
    }
    onclose(false);
  }

  async function remove() {
    if (deleting) return;
    clearTimeout(typing);
    typing = undefined;
    deleting = true;
    const result = await tabs.deleteSpace(id);
    deleting = false;
    if (result.outcome === "failed" || result.outcome === "rejected") failed = true;
    else onclose(true);
  }

  const appTheme = $derived(
    palette.resolveTheme(preferences.value("ui.theme"), preferences.value("ui.theme-custom")),
  );
  // The app's own custom theme can be worn by a space too.
  const customSeeds = $derived(
    chosen?.kind === "custom"
      ? chosen.seeds
      : appTheme.custom
        ? preferences.value("ui.theme-custom")
        : null,
  );
  const mode = () =>
    document.documentElement.dataset.theme === "light" ? ("light" as const) : ("dark" as const);
  const keyOf = (value: SpaceThemeView | null) =>
    value === null ? "" : value.kind === "preset" ? `preset:${value.id}` : `custom:${value.seeds}`;
  const same = (a: SpaceThemeView | null, b: SpaceThemeView | null) => keyOf(a) === keyOf(b);
  const themeName = $derived(
    chosen === null
      ? m.space_theme_none()
      : chosen.kind === "preset"
        ? (palette.preset(chosen.id)?.name ?? chosen.id)
        : m.space_theme_custom(),
  );

  // The window wears the chosen theme while the editor is open, even for a
  // space that is not the one shown.
  $effect(() => {
    const next = chosen;
    if (next === null) appearance.previewTheme(appTheme.spec);
    else if (next.kind === "preset")
      appearance.previewTheme(palette.forkOf(palette.preset(next.id) ?? palette.presets[0]!));
    else appearance.previewTheme(palette.parseTheme(next.seeds));
  });
  // Heard window-wide and first, since WebKit does not focus a pressed button.
  $effect(() => {
    const escape = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || event.isComposing || event.keyCode === 229) return;
      event.preventDefault();
      event.stopPropagation();
      if (confirming) confirming = false;
      else restore();
    };
    window.addEventListener("keydown", escape, true);
    return () => window.removeEventListener("keydown", escape, true);
  });
  onMount(() => {
    if (fresh) {
      field?.focus();
      field?.select();
    }
    return () => {
      if (typing !== undefined) void commit();
      appearance.previewTheme(null);
    };
  });

  const closing = (count: number) =>
    count === 0
      ? m.space_delete_body_none()
      : count === 1
        ? m.space_delete_body_one()
        : m.space_delete_body_many({ count: String(count) });
</script>

<div class="editor" data-space-hue={color}>
  <form class="editor-form" aria-label={m.space_edit_title()} onsubmit={done}>
    <div class="editor-body">
      <input
        bind:this={field}
        class="editor-name"
        value={name}
        oninput={(event) => typeName(event.currentTarget.value)}
        onblur={() => typing !== undefined && void commit()}
        maxlength={48}
        placeholder={named}
        aria-label={m.space_name()}
        spellcheck="false"
        autocomplete="off"
      />

      <div class="group" role="radiogroup" aria-labelledby="space-icon-label">
        <div class="group-head">
          <span id="space-icon-label">{m.space_icon()}</span>
          <input
            class="emoji-field"
            value={emoji}
            oninput={(event) => typeEmoji(event.currentTarget.value)}
            onfocus={() => icon?.kind !== "emoji" && emojiOf(emoji) && typeEmoji(emoji)}
            placeholder={m.space_emoji_short()}
            aria-label={m.space_emoji_placeholder()}
            data-on={icon?.kind === "emoji" || undefined}
            maxlength={16}
          />
        </div>
        <div class="symbols">
          <button
            type="button"
            role="radio"
            class="symbol symbol-none"
            aria-checked={icon === null}
            aria-label={m.space_icon_none()}
            title={m.space_icon_none()}
            onclick={() => change(() => (icon = null))}
            ><Icon icon={UnavailableIcon} size={15} strokeWidth={1.6} /></button
          >
          {#each SPACE_SYMBOLS as symbol, index (index)}
            <button
              type="button"
              role="radio"
              class="symbol"
              aria-checked={icon?.kind === "symbol" && icon.index === index}
              aria-label={m.space_symbol({ index: String(index + 1) })}
              onclick={() => change(() => (icon = { kind: "symbol", index }))}
              ><Icon icon={symbol} size={15} strokeWidth={1.8} /></button
            >
          {/each}
        </div>
      </div>

      <div class="group" role="radiogroup" aria-labelledby="space-color-label">
        <div class="group-head"><span id="space-color-label">{m.space_color()}</span></div>
        <div class="colors">
          {#each Array.from({ length: SPACE_COLORS }, (_, hue) => hue) as hue (hue)}
            <button
              type="button"
              role="radio"
              class="color"
              data-space-hue={hue}
              aria-checked={color === hue}
              aria-label={m.space_color_index({ index: String(hue + 1) })}
              onclick={() => change(() => (color = hue))}
            ></button>
          {/each}
        </div>
      </div>

      <div class="group" role="radiogroup" aria-labelledby="space-theme-label">
        <div class="group-head">
          <span id="space-theme-label">{m.space_theme()}</span>
          <span class="group-value">{themeName}</span>
        </div>
        <div class="themes">
          <button
            type="button"
            role="radio"
            class="theme theme-none"
            aria-checked={chosen === null}
            title={m.space_theme_follows()}
            aria-label={m.space_theme_none()}
            onclick={() => change(() => (chosen = null))}
            ><Icon icon={UnavailableIcon} size={14} strokeWidth={1.6} /></button
          >
          {#snippet chip(label: string, value: SpaceThemeView, seeds: palette.Seeds)}
            <button
              type="button"
              role="radio"
              class="theme"
              aria-checked={same(chosen, value)}
              title={label}
              aria-label={label}
              style:--chip-bg={seeds.bg}
              style:--chip-fg={seeds.fg}
              style:--chip-accent={seeds.accent}
              onclick={() => change(() => (chosen = value))}
            >
              <span class="theme-swatch"><i></i></span>
              {#if same(chosen, value)}<span class="theme-tick"
                  ><Icon icon={Tick02Icon} size={9} strokeWidth={2.8} /></span
                >{/if}
            </button>
          {/snippet}
          {#each palette.presets as entry (entry.id)}
            {@render chip(entry.name, { kind: "preset", id: entry.id }, entry[mode()])}
          {/each}
          {#if customSeeds}{@const spec = palette.parseTheme(customSeeds)}{#if spec}{@render chip(
                m.space_theme_custom(),
                { kind: "custom", seeds: customSeeds },
                spec[mode()],
              )}{/if}{/if}
        </div>
      </div>

      {#if failed}<p class="editor-error" role="alert">{m.space_save_failed()}</p>{/if}
    </div>

    {#if confirming}
      <div
        class="editor-foot confirm"
        role="alertdialog"
        aria-label={m.space_delete_title({ name: named })}
      >
        <div class="confirm-text">
          <p class="confirm-title">{m.space_delete_title({ name: named })}</p>
          <p class="confirm-body">{closing(space.tabs)}</p>
        </div>
        <div class="editor-actions">
          <Button size="compact" variant="secondary" onclick={() => (confirming = false)}
            >{m.space_cancel()}</Button
          >
          <Button size="compact" variant="danger" pending={deleting} onclick={remove}
            >{m.space_delete_confirm()}</Button
          >
        </div>
      </div>
    {:else}
      <div class="editor-foot editor-actions">
        {#if deletable}<Button
            size="compact"
            variant="ghost"
            class="editor-delete"
            onclick={() => (confirming = true)}>{m.space_delete()}</Button
          >{/if}
        <Button size="compact" variant="primary" type="submit">{m.space_done()}</Button>
      </div>
    {/if}
  </form>
</div>

<style>
  .editor {
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
    animation: editor-in var(--motion-base) var(--ease-out);
  }

  .editor-form {
    display: contents;
  }

  .editor-body {
    display: flex;
    flex-direction: column;
    gap: 14px;
    min-height: 0;
    overflow-y: auto;
    overscroll-behavior: contain;
    padding: 12px 12px 4px;
  }

  .editor-name {
    min-width: 0;
    height: 32px;
    padding-inline: 10px;
    border: 0;
    border-radius: var(--radius-inset);
    background: var(--row-hover);
    box-shadow: var(--row-rim);
    color: var(--color-text);
    font: inherit;
    font-size: var(--text-body);
    font-weight: 550;
    outline: none;
    transition:
      background-color var(--motion-fast) var(--ease-out),
      box-shadow var(--motion-fast) var(--ease-out);
  }

  .editor-name:focus {
    background: var(--row-active);
    box-shadow: var(--shadow-field-focus);
  }

  .editor-name::placeholder,
  .emoji-field::placeholder {
    color: var(--color-faint);
  }

  .group {
    display: flex;
    flex-direction: column;
    gap: 6px;
    min-width: 0;
  }

  .group-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    min-height: 20px;
    padding-inline: 2px;
    color: var(--color-faint);
    font-size: var(--text-caption);
    font-weight: 550;
    letter-spacing: 0.01em;
  }

  .group-value {
    overflow: hidden;
    color: var(--color-muted);
    font-weight: 450;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .emoji-field {
    width: 64px;
    height: 20px;
    padding-inline: 8px;
    border: 0;
    border-radius: var(--radius-capsule);
    background: var(--row-hover);
    color: var(--color-text);
    font: inherit;
    font-size: var(--text-label);
    text-align: center;
    outline: none;
  }

  .emoji-field:focus {
    background: var(--row-active);
  }

  .emoji-field[data-on] {
    background: var(--row-active);
    box-shadow: inset 0 0 0 1px var(--space-hue);
  }

  .symbols {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(24px, 1fr));
    gap: 1px;
  }

  .symbol {
    display: grid;
    height: 26px;
    padding: 0;
    border: 0;
    border-radius: var(--radius-inset);
    background: transparent;
    color: var(--color-muted);
    place-items: center;
    transition:
      background-color var(--motion-instant) var(--ease-out),
      color var(--motion-instant) var(--ease-out);
  }

  .symbol-none {
    color: var(--color-faint);
  }

  .symbol:hover,
  .symbol:focus-visible {
    outline: none;
    background: var(--row-hover);
    color: var(--color-text);
  }

  .symbol[aria-checked="true"] {
    background: var(--row-active);
    color: var(--space-ink);
  }

  .symbol-none[aria-checked="true"] {
    color: var(--color-text);
  }

  .colors {
    display: flex;
    justify-content: space-between;
    gap: 4px;
    padding-inline: 3px;
  }

  .color {
    flex: 0 1 16px;
    min-width: 10px;
    aspect-ratio: 1;
    padding: 0;
    border: 0;
    border-radius: 50%;
    background: var(--space-hue);
    outline: 1.5px solid transparent;
    outline-offset: 2px;
    transition:
      scale var(--motion-fast) var(--ease-spring),
      outline-color var(--motion-fast) var(--ease-out);
  }

  .color[data-space-hue="0"] {
    background: var(--avatar-0-top);
  }

  .color:hover {
    scale: 1.12;
  }

  .color[aria-checked="true"] {
    outline-color: var(--space-hue);
  }

  .color[data-space-hue="0"][aria-checked="true"] {
    outline-color: var(--avatar-0-top);
  }

  .color:focus-visible {
    outline-color: var(--color-text);
  }

  .themes {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(20px, 1fr));
    gap: 6px 4px;
    justify-items: center;
  }

  .theme {
    position: relative;
    display: grid;
    width: 20px;
    height: 20px;
    padding: 0;
    border: 0;
    border-radius: 50%;
    background: transparent;
    outline: 1.5px solid transparent;
    outline-offset: 2px;
    place-items: center;
    transition: outline-color var(--motion-fast) var(--ease-out);
  }

  .theme-none {
    box-shadow: inset 0 0 0 1px var(--color-border);
    color: var(--color-faint);
  }

  /* The theme's ground with its accent rising in the corner: what the window
     will be, at the size of a fingertip. */
  .theme-swatch {
    position: relative;
    width: 100%;
    height: 100%;
    overflow: hidden;
    border-radius: 50%;
    background: var(--chip-bg);
    box-shadow: inset 0 0 0 1px color-mix(in oklab, var(--chip-fg) 16%, transparent);
    transition: scale var(--motion-fast) var(--ease-spring);
  }

  .theme-swatch i {
    position: absolute;
    width: 55%;
    height: 55%;
    border-radius: 50%;
    background: var(--chip-accent);
    inset-block-end: -12%;
    inset-inline-end: -12%;
  }

  .theme:hover .theme-swatch {
    scale: 1.08;
  }

  .theme[aria-checked="true"] {
    outline-color: var(--color-text);
  }

  .theme-none[aria-checked="true"] {
    color: var(--color-text);
  }

  .theme:focus-visible {
    outline-color: var(--color-text);
  }

  .theme-tick {
    position: absolute;
    display: grid;
    width: 12px;
    height: 12px;
    border-radius: 50%;
    background: var(--chip-fg);
    color: var(--chip-bg);
    place-items: center;
  }

  .editor-error {
    color: var(--color-danger);
    font-size: var(--text-label);
  }

  .editor-foot {
    flex: none;
    padding: 10px 12px 12px;
  }

  .editor-actions {
    display: flex;
    flex-wrap: wrap;
    justify-content: flex-end;
    gap: 6px;
  }

  .editor-actions :global(.editor-delete) {
    margin-inline-end: auto;
    color: var(--color-danger);
  }

  .confirm {
    display: flex;
    flex-direction: column;
    gap: 10px;
    margin: 4px 8px 8px;
    padding: 10px;
    border-radius: var(--radius-inset);
    background: color-mix(in oklab, var(--color-danger) 10%, transparent);
  }

  .confirm-title {
    color: var(--color-text);
    font-size: var(--text-body);
    font-weight: 600;
  }

  .confirm-body {
    color: var(--color-muted);
    font-size: var(--text-label);
    line-height: 16px;
  }

  @keyframes editor-in {
    from {
      opacity: 0;
      translate: 6px 0;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .editor {
      animation: none;
    }

    .color,
    .theme-swatch {
      transition: none;
    }
  }

  @media (forced-colors: active) {
    .color[aria-checked="true"],
    .theme[aria-checked="true"] {
      outline-color: Highlight;
    }
  }
</style>
