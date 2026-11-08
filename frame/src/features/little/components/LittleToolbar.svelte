<script lang="ts">
  import {
    ArrowDown01Icon,
    ArrowLeft02Icon,
    ArrowRight02Icon,
    LinkSquare02Icon,
    Refresh01Icon,
  } from "@hugeicons/core-free-icons";
  import type { LittleIntent, TabView } from "$shared/ipc/bindings";
  import * as m from "$shared/i18n/messages";
  import IconButton from "$shared/ui/IconButton";
  import { hostOf } from "../lib/little-model";

  let {
    tab,
    spaces,
    onAction,
    onSpaceMenu,
  }: {
    tab: TabView | null;
    /** How many spaces the page can go to; a choice is offered past one. */
    spaces: number;
    onAction: (intent: LittleIntent) => void;
    onSpaceMenu: (x: number, y: number) => void;
  } = $props();

  function chooseSpace(event: MouseEvent) {
    const rect = (event.currentTarget as HTMLElement).getBoundingClientRect();
    onSpaceMenu(rect.left, rect.bottom + 4);
  }
</script>

<!-- The page's title and host are untrusted; they are only ever text. -->
<header class="little-toolbar" data-tauri-drag-region="deep">
  <div class="flex items-center gap-0.5" role="group" aria-label={m.ui_navigation()}>
    <IconButton
      icon={ArrowLeft02Icon}
      label={m.ui_back()}
      disabled={tab?.can_go_back !== true}
      onclick={() => onAction("back")}
    />
    <IconButton
      icon={ArrowRight02Icon}
      label={m.ui_forward()}
      disabled={tab?.can_go_forward !== true}
      onclick={() => onAction("forward")}
    />
    <IconButton
      icon={Refresh01Icon}
      label={m.ui_reload()}
      disabled={!tab}
      onclick={() => onAction("reload")}
    />
  </div>
  {#if tab}
    <button
      type="button"
      class="little-page"
      title={tab.url ?? undefined}
      data-zephium-little-tab={tab.id}
      data-zephium-tab-url={tab.url ?? ""}
      data-zephium-projection-revision={tab.projection_revision}
      onclick={() => onAction("edit_address")}
    >
      <span class="little-title" data-zephium-tab-label>{tab.title}</span>
      <span class="little-host" data-zephium-address>{hostOf(tab.url)}</span>
    </button>
  {/if}
  <span class="flex-1" aria-hidden="true"></span>
  <div class="flex items-center">
    <IconButton
      icon={LinkSquare02Icon}
      label={m.little_open_in_main()}
      disabled={!tab}
      onclick={() => onAction("open_in_main")}
    />
    {#if tab && spaces > 1}<IconButton
        icon={ArrowDown01Icon}
        label={m.native_move_to_space()}
        size={14}
        buttonSize={20}
        haspopup="menu"
        onclick={chooseSpace}
      />{/if}
  </div>
</header>

<style>
  /* The lights, then the page's controls, clear of the wider spacing of
     recent macOS lights. */
  .little-toolbar {
    display: flex;
    align-items: center;
    gap: 6px;
    height: 100%;
    padding-inline: 76px 8px;
  }

  /* Only as wide as its text, so the rest of the row still drags the window. */
  .little-page {
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
  }

  .little-page:hover {
    background: var(--color-control-hover);
  }

  .little-title {
    overflow: hidden;
    font-weight: 500;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .little-host {
    flex: none;
    max-width: 50%;
    overflow: hidden;
    color: var(--color-muted);
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
