<script lang="ts">
  import {
    ArrowDown01Icon,
    ArrowLeft02Icon,
    ArrowRight02Icon,
    Cancel01Icon,
    MinusSignIcon,
    PinIcon,
    Refresh01Icon,
  } from "@hugeicons/core-free-icons";
  import type { IconSvgElement } from "@hugeicons/svelte";
  import type { MiniIntent, TabView } from "$shared/ipc/bindings";
  import * as m from "$shared/i18n/messages";
  import Icon from "$shared/ui/Icon";
  import { hostOf } from "../lib/mini-model";

  let {
    tab,
    spaces,
    pinned,
    frame,
    onAction,
    onSpaceMenu,
  }: {
    tab: TabView | null;
    /** How many spaces the page can go to; a choice is offered past one. */
    spaces: number;
    /** Whether the window stays above other windows. */
    pinned: boolean;
    /** macOS draws the lights at the start of the row; Windows draws no
     *  frame, so the row carries the window's own controls at its end. */
    frame: "lights" | "controls";
    onAction: (intent: MiniIntent) => void;
    onSpaceMenu: (x: number, y: number) => void;
  } = $props();

  function chooseSpace(event: MouseEvent) {
    const rect = (event.currentTarget as HTMLElement).getBoundingClientRect();
    onSpaceMenu(rect.left, rect.bottom + 4);
  }
</script>

<!-- The page's title and host are untrusted; they are only ever text. -->
<header class="mini-toolbar" data-frame={frame} data-tauri-drag-region="deep">
  <!-- Plain chrome buttons: the toolbar shares only the smallest controls
       with the browser, so neither page pays for another shared chunk. -->
  {#snippet control(icon: IconSvgElement, label: string, intent: MiniIntent, disabled: boolean)}
    <button
      type="button"
      class="chrome-button"
      aria-label={label}
      title={label}
      {disabled}
      onclick={() => onAction(intent)}
    >
      <Icon {icon} size={16} />
    </button>
  {/snippet}
  <div class="flex items-center gap-0.5" role="group" aria-label={m.ui_navigation()}>
    {@render control(ArrowLeft02Icon, m.ui_back(), "back", tab?.can_go_back !== true)}
    {@render control(ArrowRight02Icon, m.ui_forward(), "forward", tab?.can_go_forward !== true)}
    {@render control(Refresh01Icon, m.ui_reload(), "reload", !tab)}
  </div>
  {#if tab}
    <button
      type="button"
      class="mini-page"
      title={tab.url ?? undefined}
      data-zephium-mini-tab={tab.id}
      data-zephium-tab-url={tab.url ?? ""}
      data-zephium-projection-revision={tab.projection_revision}
      onclick={() => onAction("edit_address")}
    >
      <span class="mini-title" data-zephium-tab-label>{tab.title}</span>
      <span class="mini-host" data-zephium-address>{hostOf(tab.url)}</span>
    </button>
  {/if}
  <span class="flex-1" aria-hidden="true"></span>
  <button
    type="button"
    class={["chrome-button", pinned && "chrome-button-active"]}
    aria-label={m.mini_keep_on_top()}
    title={m.mini_keep_on_top()}
    aria-pressed={pinned}
    onclick={() => onAction("keep_on_top")}
  >
    <Icon icon={PinIcon} size={15} />
  </button>
  <!-- One gesture: the whole button moves the page into the browser, and
       the chevron beside it picks which space it lands in. -->
  <div class="mini-handoff">
    <button
      type="button"
      class="mini-open"
      disabled={!tab}
      title={m.mini_open_in_main_hint()}
      onclick={() => onAction("open_in_main")}
    >
      {m.mini_open_in_main()}
    </button>
    {#if tab && spaces > 1}
      <button
        type="button"
        class="mini-space"
        aria-label={m.mini_choose_space()}
        title={m.mini_choose_space()}
        aria-haspopup="menu"
        onclick={chooseSpace}
      >
        <Icon icon={ArrowDown01Icon} size={12} />
      </button>
    {/if}
  </div>
  {#if frame === "controls"}
    <div class="mini-window-controls">
      <button
        type="button"
        class="window-button"
        aria-label={m.window_minimize()}
        title={m.window_minimize()}
        onclick={() => onAction("minimize")}
      >
        <Icon icon={MinusSignIcon} size={13} />
      </button>
      <button
        type="button"
        class="window-button window-button-close"
        aria-label={m.window_close()}
        title={m.window_close()}
        onclick={() => onAction("close")}
      >
        <Icon icon={Cancel01Icon} size={13} />
      </button>
    </div>
  {/if}
  {#if tab?.loading}
    <span class="mini-loading" role="progressbar" aria-label={m.surface_loading()}></span>
  {/if}
</header>

<style>
  .mini-toolbar {
    position: relative;
    display: flex;
    align-items: center;
    gap: 6px;
    height: 100%;
    padding-inline: 8px;
  }

  /* The lights, then the page's controls, clear of the wider spacing of
     recent macOS lights. */
  .mini-toolbar[data-frame="lights"] {
    padding-inline-start: 76px;
  }

  /* The window's own buttons sit flush with its corner, as Windows draws them. */
  .mini-toolbar[data-frame="controls"] {
    padding-inline-end: 0;
  }

  .chrome-button:disabled {
    color: var(--color-faint);
    background: transparent;
    opacity: 0.45;
  }

  /* Only as wide as its text, so the rest of the row still drags the window. */
  .mini-page {
    display: flex;
    flex: 0 1 auto;
    min-width: 0;
    align-items: baseline;
    gap: 8px;
    height: 28px;
    padding: 0 8px;
    line-height: 28px;
    color: inherit;
    text-align: start;
    background: none;
    border: 0;
    border-radius: var(--radius-control-compact);
    transition: background-color var(--motion-instant) var(--ease-smooth);
  }

  .mini-page:hover {
    background: var(--color-control-hover);
  }

  .mini-title {
    overflow: hidden;
    font-weight: 500;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .mini-host {
    flex: none;
    max-width: 50%;
    overflow: hidden;
    color: var(--color-muted);
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  /* The way out of the mini window is its one labelled control. */
  .mini-handoff {
    display: flex;
    flex: none;
    align-items: stretch;
    height: 26px;
    overflow: hidden;
    color: var(--color-on-control);
    background: var(--color-control);
    border-radius: var(--radius-control-compact);
    box-shadow: var(--shadow-control);
  }

  .mini-open,
  .mini-space {
    display: flex;
    align-items: center;
    color: inherit;
    white-space: nowrap;
    background: none;
    border: 0;
    transition: background-color var(--motion-instant) var(--ease-smooth);
  }

  .mini-open {
    padding: 0 10px;
    font-weight: 500;
  }

  .mini-space {
    padding: 0 6px;
    border-inline-start: 1px solid var(--color-border);
  }

  .mini-open:disabled {
    color: var(--color-muted);
  }

  .mini-open:focus-visible,
  .mini-space:focus-visible {
    outline-offset: -2px;
  }

  .mini-open:hover:not(:disabled),
  .mini-space:hover {
    background: var(--color-control-hover);
  }

  .mini-open:active:not(:disabled),
  .mini-space:active {
    background: var(--color-control-pressed);
  }

  .mini-window-controls {
    display: flex;
    align-self: stretch;
    align-items: stretch;
    margin-inline-start: 2px;
  }

  .mini-window-controls .window-button {
    height: 100%;
    width: 44px;
    border-radius: 0;
  }

  /* A thin line under the toolbar while the page loads. With reduced motion
     it stays still: the line alone says the page is loading. */
  .mini-loading {
    position: absolute;
    inset: auto 0 0;
    height: 2px;
    overflow: hidden;
    pointer-events: none;
  }

  .mini-loading::after {
    position: absolute;
    inset: 0;
    width: 40%;
    content: "";
    background: var(--color-accent);
    border-radius: var(--radius-capsule);
    animation: mini-loading 1.2s var(--ease-smooth) infinite;
  }

  @keyframes mini-loading {
    from {
      translate: -100% 0;
    }

    to {
      translate: 250% 0;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .mini-loading::after {
      width: 100%;
      opacity: 0.6;
      animation: none;
    }
  }

  :global(:root[data-reduce-motion="true"]) .mini-loading::after {
    width: 100%;
    opacity: 0.6;
    animation: none;
  }

  @media (forced-colors: active) {
    .mini-handoff {
      border: 1px solid ButtonText;
    }

    .mini-loading::after {
      background: Highlight;
    }
  }
</style>
