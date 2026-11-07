<script lang="ts">
  import * as m from "$shared/i18n/messages";
  import { Cancel01Icon, Globe02Icon } from "@hugeicons/core-free-icons";
  import type { TabView } from "$shared/ipc/bindings";
  import FavIcon from "$shared/ui/FavIcon";
  import Icon from "$shared/ui/Icon";
  import { favicons } from "$domain/favicons";
  import CaptureControl from "$shared/ui/CaptureControl";
  import AudioControl from "$shared/ui/AudioControl";
  import { tabs } from "$domain/tabs";

  let {
    tab,
    active,
    splitCandidate,
    class: className = "",
    onSelect,
    onContextMenu,
    onPointerDown,
    onPointerMove,
    onPointerUp,
    onPointerCancel,
  }: {
    tab: TabView;
    active: boolean;
    splitCandidate: boolean;
    class?: string;
    onSelect: (id: string) => void;
    onContextMenu: (event: MouseEvent, tab: TabView) => void;
    onPointerDown: (event: PointerEvent, tab: TabView) => void;
    onPointerMove: (event: PointerEvent) => void;
    onPointerUp: (event: PointerEvent) => void;
    onPointerCancel: (event: PointerEvent) => void;
  } = $props();

  // An Essential with no page behind it only keeps its site; one that has a
  // page can be closed without being let go.
  let open = $derived(tab.availability?.state !== "sleeping");
  let name = $derived(tab.title || m.untitled_tab());

  function closePage(event: MouseEvent) {
    event.stopPropagation();
    tabs.close(tab.id);
  }
</script>

<!--
  Essentials share the row's width evenly, so one kept site fills the row and
  four divide it. The tile is wide rather than square: a wide plate leaves the
  mark plenty of quiet ground around it, which is what keeps a handful of
  unrelated brand colours from reading as a block of noise.
-->
<!--
  The presentation sentinels belong to the row, not to the tile's styling.
  Native resolves the tab by id and verifies its url and revision through
  this element before it will reveal page content, so a tile without them
  activates nothing at all.
-->
<li
  data-motion-key={`tab:${tab.id}`}
  data-zephium-tab-id={tab.id}
  data-zephium-tab-url={tab.url ?? ""}
  data-zephium-projection-revision={tab.projection_revision}
  data-audio-host
  class={["essential", className]}
  data-open={open}
  data-active={active}
  data-split-candidate={splitCandidate}
>
  <button
    type="button"
    data-plate
    aria-current={active ? "page" : undefined}
    aria-label={name}
    title={name}
    oncontextmenu={(event) => onContextMenu(event, tab)}
    onpointerdown={(event) => onPointerDown(event, tab)}
    onpointermove={onPointerMove}
    onpointerup={onPointerUp}
    onpointercancel={onPointerCancel}
    onclick={() => onSelect(tab.id)}
  >
    <FavIcon
      image={favicons.image(tab.icon)}
      tone={favicons.tone(tab.icon)}
      loading={tab.loading}
      size={18}
      lit
      fallback={Globe02Icon}
      notched={!!tab.audio}
    />
    <span data-zephium-tab-label class="sr-only">{tab.title}</span>
  </button>
  {#if tab.audio}
    <AudioControl
      audio={tab.audio}
      size={18}
      onToggle={(muted) => void tabs.setMuted(tab.id, muted)}
    />
  {/if}
  {#if tab.capture}
    <span class="capture-badge">
      <CaptureControl
        badge
        onOpen={(x, y) => tabs.openMediaMenu(tab.id, x, y)}
        site={tab.url ?? tab.title}
        capture={tab.capture}
      />
    </span>
  {/if}
  {#if open}
    <button
      type="button"
      class="essential-close"
      aria-label={m.close_named_tab({ title: name })}
      onclick={closePage}
    >
      <Icon icon={Cancel01Icon} size={10} strokeWidth={2.2} />
    </button>
  {/if}
</li>

<style>
  .essential {
    position: relative;
    min-width: 0;
  }

  .capture-badge {
    position: absolute;
    inset-inline-end: 2px;
    inset-block-end: 2px;
  }

  /* A plate of quiet ground and nothing else. A drawn ring made every tile
     read as a button; the fill alone already separates it from the column,
     and a row of unrelated brand marks stays calm on it. */
  .essential > button[data-plate] {
    display: grid;
    place-items: center;
    width: 100%;
    height: var(--dock-tile);
    border: 0;
    border-radius: var(--radius-card);
    background: var(--color-tile);
    box-shadow: var(--shine), var(--shadow-tile);
    cursor: default;
    transition:
      background-color var(--motion-fast) var(--ease-out),
      box-shadow var(--motion-fast) var(--ease-out),
      scale var(--motion-slow) var(--ease-spring);
  }

  .essential > button[data-plate]:hover {
    background: var(--color-tile-hover);
  }

  .essential > button[data-plate]:active {
    scale: 0.95;
    transition-duration: var(--motion-instant);
  }

  .essential > button[data-plate][aria-current="page"] {
    background: var(--color-fill-active);
    box-shadow: var(--row-rim);
  }

  .essential[data-split-candidate="true"] > button[data-plate] {
    box-shadow: inset 0 0 0 1px var(--color-accent);
  }

  /* A dot under the mark says a page is open behind the tile, and the plate
     lit as it is for the current tab says which one is in front. A closed
     Essential has neither: it is only the site. */
  .essential[data-open="true"]::after {
    content: "";
    position: absolute;
    inset-inline: 0;
    inset-block-end: 4px;
    width: 4px;
    height: 4px;
    margin-inline: auto;
    border-radius: var(--radius-capsule);
    background: var(--color-faint);
    pointer-events: none;
  }

  .essential[data-active="true"]::after {
    background: var(--color-label-secondary);
  }

  /* Ends the page, never the Essential. It hangs in the corner rather than
     taking room from the mark, and shows for a hand or the keyboard alike. */
  .essential-close {
    position: absolute;
    inset-block-start: 4px;
    inset-inline-end: 4px;
    z-index: 2;
    display: grid;
    place-items: center;
    width: 16px;
    height: 16px;
    padding: 0;
    border: 0;
    border-radius: var(--radius-capsule);
    background: var(--color-fill-hover);
    color: var(--color-muted);
    opacity: 0;
    cursor: default;
    transition:
      opacity var(--motion-fast) var(--ease-out),
      background-color var(--motion-fast) var(--ease-out),
      color var(--motion-fast) var(--ease-out);
  }

  .essential-close:hover {
    background: var(--color-fill-pressed);
    color: var(--color-text);
  }

  .essential-close:focus-visible,
  .essential:is(:hover, :focus-within) .essential-close {
    opacity: 1;
  }

  @media (forced-colors: active) {
    .essential[data-open="true"]::after {
      background: CanvasText;
    }

    .essential-close {
      border: 1px solid ButtonText;
      background: ButtonFace;
      color: ButtonText;
    }
  }
</style>
