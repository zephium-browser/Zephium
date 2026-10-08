<script lang="ts">
  import * as m from "$shared/i18n/messages";
  import { Globe02Icon } from "@hugeicons/core-free-icons";
  import type { TabView } from "$shared/ipc/bindings";
  import FavIcon from "$shared/ui/FavIcon";
  import { favicons } from "$domain/favicons";
  import CaptureControl from "$shared/ui/CaptureControl";
  import { stopCaptureFor } from "$domain/capture";

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
  class={["essential", className]}
  data-split-candidate={splitCandidate}
>
  <button
    type="button"
    data-plate
    aria-current={active ? "page" : undefined}
    aria-label={tab.title || m.untitled_tab()}
    title={tab.title || m.untitled_tab()}
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
    />
    <span data-zephium-tab-label class="sr-only">{tab.title}</span>
  </button>
  {#if tab.capture}
    <span class="capture-badge">
      {#key tab.capture.navigation_id}<CaptureControl
          onStop={stopCaptureFor(tab.id, tab.capture.navigation_id)}
          site={tab.url ?? tab.title}
          capture={tab.capture}
        />{/key}
    </span>
  {/if}
</li>

<style>
  .essential {
    position: relative;
    min-width: 0;
  }

  .capture-badge {
    position: absolute;
    inset-inline-end: 0;
    inset-block-end: 0;
    background: var(--color-raised);
    border-radius: var(--radius-capsule);
  }

  /* A plate of quiet ground and nothing else. A drawn ring made every tile
     read as a button; the fill alone already separates it from the column,
     and a row of unrelated brand marks stays calm on it. */
  .essential > button {
    display: grid;
    place-items: center;
    width: 100%;
    height: var(--dock-tile);
    border: 0;
    border-radius: var(--radius-card);
    background: var(--color-card);
    cursor: default;
    transition:
      background-color var(--motion-fast) var(--ease-out),
      box-shadow var(--motion-fast) var(--ease-out),
      scale var(--motion-slow) var(--ease-spring);
  }

  .essential > button:hover {
    background: var(--color-fill-hover);
  }

  .essential > button:active {
    scale: 0.95;
    transition-duration: var(--motion-instant);
  }

  .essential > button[aria-current="page"] {
    background: var(--color-fill-active);
    box-shadow: var(--row-rim);
  }

  .essential[data-split-candidate="true"] > button {
    box-shadow: inset 0 0 0 1px var(--color-accent);
  }
</style>
