<script lang="ts">
  import { scale, slide } from "svelte/transition";
  import { backOut } from "svelte/easing";
  import {
    ArrowDown01Icon,
    FlashIcon,
    Globe02Icon,
    Search01Icon,
    Settings02Icon,
  } from "@hugeicons/core-free-icons";
  import * as m from "$shared/i18n/messages";
  import type { SearchShortcut } from "$shared/ipc/bindings";
  import { preferences } from "$domain/preferences";
  import { favicons } from "$domain/favicons";
  import type { MenuEntry } from "$shared/ui/Menu";
  import Icon from "$shared/ui/Icon";
  import FavIcon from "$shared/ui/FavIcon";
  import EngineMark, { engineMarkUrl } from "$shared/ui/EngineMark";
  import { reducedMotion } from "$shared/lib/motion";
  import { shortcutFor, shortcutKeyword, withShortcut } from "$shared/lib/search-shortcuts";

  type MenuModule = typeof import("$shared/ui/Menu");

  let {
    query,
    awake,
    profile,
    loadMenu,
    size = 18,
    onpick,
    onsettings,
  }: {
    /** Opens where the shortcuts are kept, when the page offers it. */
    onsettings?: () => void;
    /** Whose icons to ask for; a private window asks for none. */
    profile: { id: string; kind: string } | null;
    /** The menu, loaded by whoever composes the page, so its machinery stays
     *  out of every graph that only shares this field. */
    loadMenu?: () => Promise<MenuModule>;
    query: string;
    /** The field has the focus or holds a query: the mark says where it goes. */
    awake: boolean;
    size?: number;
    /** A new query to put in the field, or null when only the engine changed. */
    onpick: (query: string | null) => void;
  } = $props();

  /** The engines a search can go to by default, which are shortcuts too. */
  const ENGINES = ["google", "duckduckgo", "bing", "brave"];

  let shortcuts = $derived(preferences.searchShortcuts());
  let engine = $derived(preferences.value("search.engine"));
  let named = $derived(shortcutFor(query, shortcuts));
  let fallback = $derived(shortcuts.find((entry) => entry.id === engine) ?? null);
  let open = $state(false);
  // The menu and its machinery are not part of the page's first load: they
  // arrive as the pointer or the focus reaches the mark, before it is pressed.
  let MenuView = $state<MenuModule["default"] | null>(null);
  let loading: Promise<void> | null = null;
  let host = $state<HTMLElement>();
  function load(opening = false) {
    if (!loadMenu) return;
    loading ??= loadMenu()
      .then((module) => {
        const focused = host?.contains(document.activeElement) ?? false;
        MenuView = module.default;
        if (focused) queueMicrotask(() => host?.querySelector<HTMLElement>(".engine")?.focus());
      })
      .catch(() => {
        loading = null;
      });
    if (opening) void loading.then(() => (open = true));
  }
  /** The site the query is headed for: one it names, else the engine. */
  let shown = $derived(named ?? (awake || open ? fallback : null));

  // The chevron coming or going moves the field's text. The system draws
  // the caret where WebKit last reported the selection, which only a change
  // of selection reports again, so the selection steps away and back.
  function recaret() {
    const field = host?.closest(".ui-search")?.querySelector("input");
    if (!field || document.activeElement !== field) return;
    const { value, selectionStart, selectionEnd, selectionDirection } = field;
    if (selectionStart === null || selectionEnd === null) return;
    field.value = `${value} `;
    field.setSelectionRange(value.length + 1, value.length + 1);
    field.value = value;
    field.setSelectionRange(selectionStart, selectionEnd, selectionDirection ?? "none");
  }

  const markOf = (entry: SearchShortcut | null) =>
    entry ? favicons.forPage(entry.home_url) : null;

  // The marks of every place a search can go are asked for once, where the
  // profile keeps what it sees; a private window probes nothing.
  $effect(() => {
    favicons.probe(
      profile,
      shortcuts.filter((entry) => entry.enabled).map((entry) => entry.home_url),
    );
  });

  // The engines first, each with its own mark; the shortcuts open beside
  // them, so the menu stays as short as the choice it mostly is.
  let entries = $derived.by<MenuEntry[]>(() => {
    const engines = shortcuts.filter((entry) => ENGINES.includes(entry.id));
    const others = shortcuts.filter((entry) => entry.enabled && !ENGINES.includes(entry.id));
    const tail: MenuEntry[] = [];
    if (others.length)
      tail.push({
        kind: "submenu",
        label: m.search_menu_shortcuts(),
        icon: FlashIcon,
        entries: others.map((entry): MenuEntry => {
          const mark = markOf(entry);
          return {
            kind: "item",
            id: `shortcut:${entry.id}`,
            label: entry.name,
            icon: Globe02Icon,
            site: { image: mark?.image ?? null, tone: mark?.tone },
            hint: shortcutKeyword(entry),
          };
        }),
      });
    if (onsettings)
      tail.push({
        kind: "item",
        id: "settings",
        label: m.search_menu_edit(),
        icon: Settings02Icon,
      });
    return [
      { kind: "heading", label: m.search_menu_engines() },
      ...engines.map((entry): MenuEntry => ({
        kind: "item",
        id: `engine:${entry.id}`,
        label: entry.name,
        icon: Globe02Icon,
        image: engineMarkUrl(entry.id),
        checked: entry.id === engine,
      })),
      ...(tail.length ? [{ kind: "separator" } as const, ...tail] : []),
    ];
  });

  function select(id: string) {
    if (id === "settings") return onsettings?.();
    const [kind, value] = id.split(/:(.*)/su) as [string, string];
    if (kind === "engine") {
      void preferences.set("search.engine", value);
      onpick(
        named && ENGINES.includes(named.id) ? withShortcut(query, "", shortcuts).trim() : null,
      );
      return;
    }
    const entry = shortcuts.find((item) => item.id === value);
    if (entry) onpick(withShortcut(query, shortcutKeyword(entry), shortcuts));
  }
</script>

{#snippet mark()}
  <span class="stack" style:--size={`${size}px`} data-mark={shown?.id ?? "search"}>
    {#key shown?.id ?? ""}
      <span
        class="mark"
        in:scale={{ start: 0.55, duration: reducedMotion() ? 0 : 220, easing: backOut }}
        out:scale={{ start: 0.8, duration: reducedMotion() ? 0 : 120 }}
      >
        {#if shown && engineMarkUrl(shown.id)}<EngineMark
            engine={shown.id}
            {size}
          />{:else if shown}<FavIcon
            image={markOf(shown)?.image ?? null}
            tone={markOf(shown)?.tone}
            {size}
            lit
            fallback={Search01Icon}
          />{:else}<Icon icon={Search01Icon} size={size - 2} />{/if}
      </span>
    {/key}
  </span>
  {#if shown}<span
      class="chevron"
      transition:slide={{ axis: "x", duration: reducedMotion() ? 0 : 180 }}
      onintroend={recaret}
      onoutroend={recaret}><Icon icon={ArrowDown01Icon} size={12} strokeWidth={2} /></span
    >{/if}
{/snippet}

<span class="host" bind:this={host}>
  {#if MenuView}
    <MenuView
      label={m.search_menu_label()}
      {entries}
      triggerClass="engine"
      contentClass="engine-menu"
      returnFocus={() => false}
      onselect={select}
      bind:open
      trigger={mark}
    />
  {:else}
    <button
      type="button"
      class="engine"
      aria-label={m.search_menu_label()}
      aria-haspopup={loadMenu ? "menu" : undefined}
      onpointerenter={() => load()}
      onfocus={() => load()}
      onclick={() => load(true)}>{@render mark()}</button
    >
  {/if}
</span>

<style>
  /* The magnifier's place, a press target a little larger than it; with a
     place named, a chevron beside the mark says it opens. */
  :global(.engine) {
    display: flex;
    flex: none;
    align-items: center;
    min-inline-size: 30px;
    block-size: 30px;
    margin-inline: -6px -4px;
    padding-inline: 6px;
    border: 0;
    border-radius: var(--radius-inset);
    background: transparent;
    color: inherit;
    transition: background-color var(--motion-instant) var(--ease-smooth);
  }

  :global(.engine:hover),
  :global(.engine[data-state="open"]) {
    background: var(--color-fill);
  }

  :global(.engine:focus-visible) {
    outline: 2px solid var(--color-ring);
    outline-offset: 0;
  }

  .host {
    display: contents;
  }

  .stack {
    display: grid;
    inline-size: var(--size);
    block-size: var(--size);
  }

  .mark {
    display: grid;
    grid-area: 1 / 1;
    place-items: center;
  }

  .chevron {
    display: grid;
    padding-inline-start: 3px;
    place-items: center;
    color: var(--color-faint);
    transition: color var(--motion-instant) var(--ease-smooth);
  }

  :global(.engine:hover) .chevron,
  :global(.engine[data-state="open"]) .chevron {
    color: var(--color-muted);
  }

  /* It opens over the page's sky, which no trace of should read through. */
  :global(.engine-menu) {
    background: color-mix(in srgb, var(--color-float) 98%, transparent);
    backdrop-filter: blur(30px) saturate(1.4);
    max-block-size: min(360px, var(--bits-dropdown-menu-content-available-height, 360px));
  }
</style>
