<script lang="ts" module>
  // Rows past the first screenful share the last beat anyway; leaving them
  // out of the entrance keeps launch from animating hundreds of layers.
  const CASCADE_ROWS = 15;
</script>

<script lang="ts">
  import * as m from "$shared/i18n/messages";
  import { Cancel01Icon, Globe02Icon, PuzzleIcon } from "@hugeicons/core-free-icons";
  import type { TabView } from "$shared/ipc/bindings";
  import FavIcon from "$shared/ui/FavIcon";
  import { favicons } from "$domain/favicons";
  import Icon from "$shared/ui/Icon";
  import CaptureControl from "$shared/ui/CaptureControl";
  import { stopCaptureFor } from "$domain/capture";
  import { closeOnMiddleClick } from "../lib/middle-click";

  let {
    cascade = 0,
    tab,
    active,
    grouped = false,
    depth = 0,
    closable = true,
    class: className = "",
    splitCandidate,
    onSelect,
    onClose,
    onContextMenu,
    onPointerDown,
    onPointerMove,
    onPointerUp,
    onPointerCancel,
  }: {
    /** This row's place in the launch cascade. */
    cascade?: number;
    tab: TabView;
    active: boolean;
    grouped?: boolean;
    depth?: number;
    closable?: boolean;
    class?: string;
    splitCandidate: boolean;
    onSelect: (id: string) => void;
    onClose: (id: string) => void;
    onContextMenu: (event: MouseEvent, tab: TabView) => void;
    onPointerDown: (event: PointerEvent, tab: TabView) => void;
    onPointerMove: (event: PointerEvent) => void;
    onPointerUp: (event: PointerEvent) => void;
    onPointerCancel: (event: PointerEvent) => void;
  } = $props();

  function closeTab(event: MouseEvent) {
    event.stopPropagation();
    onClose(tab.id);
  }

  // A tab with no page yet is a different thing from a page whose site simply
  // supplies no icon, and the row should say which.
  let fallback = $derived(
    tab.content === "extensions" || tab.content === "extension_owned" ? PuzzleIcon : Globe02Icon,
  );
</script>

<li
  data-motion-key={`tab:${tab.id}`}
  data-plate
  data-zephium-tab-id={tab.id}
  data-zephium-tab-url={tab.url ?? ""}
  data-zephium-projection-revision={tab.projection_revision}
  class={["browse-tab", grouped && "browse-tab-grouped", className]}
  data-selected={active}
  data-split-candidate={splitCandidate}
  data-cascade={cascade < CASCADE_ROWS || undefined}
  style:--cascade={cascade}
>
  <button
    type="button"
    class="tab-open"
    style:padding-inline-start={`${grouped ? 8 : 8 + Math.min(depth, 8) * 13}px`}
    aria-current={active ? "page" : undefined}
    aria-label={tab.title || m.untitled_tab()}
    oncontextmenu={(event) => onContextMenu(event, tab)}
    onpointerdown={(event) => onPointerDown(event, tab)}
    onpointermove={onPointerMove}
    onpointerup={onPointerUp}
    onpointercancel={onPointerCancel}
    onclick={() => onSelect(tab.id)}
    {...closable ? closeOnMiddleClick(() => onClose(tab.id)) : {}}
  >
    <FavIcon
      image={favicons.image(tab.icon)}
      tone={favicons.tone(tab.icon)}
      loading={tab.loading}
      size={16}
      lit={active}
      {fallback}
    />
    <span data-zephium-tab-label class="tab-label">{tab.title}</span>
  </button>
  {#if tab.capture}
    <span style:margin-inline-end={closable ? "28px" : "4px"}>
      {#key tab.capture.navigation_id}
        <CaptureControl
          site={tab.url ?? tab.title}
          capture={tab.capture}
          onStop={stopCaptureFor(tab.id, tab.capture.navigation_id)}
        />
      {/key}
    </span>
  {/if}
  {#if closable}
    <button
      type="button"
      aria-label={m.close_named_tab({ title: tab.title || m.untitled_tab() })}
      title={m.close_tab()}
      class="tab-close"
      onclick={closeTab}
    >
      <Icon icon={Cancel01Icon} size={12} />
    </button>
  {/if}
</li>

<style>
  /*
    The close control is absolutely positioned rather than reserved in the
    layout. Reserving it cost 28px of every row permanently to a button that
    is invisible until hover, which at 240px is the difference between a
    readable title and one that dies mid-word.
  */
  .browse-tab {
    position: relative;
    display: flex;
    align-items: center;
    height: var(--row-sidebar);
    border-radius: var(--radius-row);
    color: var(--color-label-secondary);
    font-size: var(--sidebar-row-text);
    font-weight: var(--sidebar-row-weight);
    letter-spacing: -0.005em;
    transition:
      background-color var(--motion-fast) var(--ease-out),
      box-shadow var(--motion-fast) var(--ease-out),
      color var(--motion-base) var(--ease-out);
  }

  .browse-tab-grouped {
    border-radius: 0;
  }

  .browse-tab:not([data-selected="true"]):hover {
    background: var(--row-hover);
    color: var(--color-text);
  }

  /* Only the current tab is fully lit, and it is the one row genuinely above
     the column. The rest sit a step back so the list reads as one surface. */
  .browse-tab[data-selected="true"] {
    background: var(--row-active);
    color: var(--color-text);
    font-weight: var(--sidebar-row-weight-current);
  }

  .browse-tab[data-selected="true"]:not(.browse-tab-grouped) {
    box-shadow: var(--row-rim);
  }

  .browse-tab[data-split-candidate="true"] {
    box-shadow: inset 0 0 0 1px var(--color-accent);
  }

  .tab-open {
    display: flex;
    flex: 1;
    align-items: center;
    align-self: stretch;
    gap: 10px;
    min-width: 0;
    padding-inline-end: 4px;
    border: 0;
    border-radius: inherit;
    background: transparent;
    color: inherit;
    font: inherit;
    text-align: start;
    cursor: default;
  }

  /* The ring sits inside the row: outside it, it would collide with the
     rows above and below at a 4px gap. */
  .tab-open:focus-visible {
    outline-offset: -2px;
  }

  /*
    A title clipped with an ellipsis reads as a failure; the same title
    dissolving into the row's edge reads as a deliberate edge. The fade is
    always applied — on a short title the gradient falls past the last glyph,
    so it costs nothing and needs no overflow measurement.
  */
  .tab-label {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    white-space: nowrap;
    mask-image: var(--mask-fade-end);
  }

  .tab-label:dir(rtl) {
    mask-image: var(--mask-fade-end-rtl);
  }

  .tab-close {
    position: absolute;
    inset-inline-end: 5px;
    display: flex;
    align-items: center;
    justify-content: center;
    width: 21px;
    height: 21px;
    border: 0;
    border-radius: var(--radius-inset);
    background: transparent;
    color: var(--color-faint);
    opacity: 0;
    cursor: default;
    transition:
      opacity var(--motion-fast) var(--ease-out),
      background-color var(--motion-fast) var(--ease-out),
      color var(--motion-fast) var(--ease-out);
  }

  .tab-close:hover {
    background: var(--row-pressed);
    color: var(--color-text);
  }

  .tab-close:focus-visible {
    opacity: 1;
  }

  .browse-tab:hover .tab-close {
    opacity: 1;
  }

  @media (forced-colors: active) {
    .tab-label {
      mask-image: none;
    }
  }
</style>
