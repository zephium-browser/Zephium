<script lang="ts">
  import "$shared/ui/Menu/surface.css";
  import * as m from "$shared/i18n/messages";
  import { Add01Icon, ArrowDown01Icon } from "@hugeicons/core-free-icons";
  import { tabs } from "$domain/tabs";
  import { uiCommands as ui } from "$domain/ui-commands";
  import Icon from "$shared/ui/Icon";
  import { untrack, type Snippet } from "svelte";
  import SpaceGlyph from "./SpaceGlyph.svelte";
  import SpaceFlowBody from "./SpaceFlowBody.svelte";
  import * as requests from "../lib/space-requests.svelte";
  import { SpaceFlow } from "../lib/space-flow.svelte";

  /** What the browser sets beside the plus: pinned extensions. */
  let { extras }: { extras?: Snippet } = $props();

  // The switcher loads the first time the chip is approached or opened, and
  // stays, so reopening is the panel's motion alone.
  let loaded = $state(false);
  const load = () => (loaded = true);
  $effect(() => {
    if (flow.open) load();
  });

  let panel = $state<HTMLElement>();
  let chip = $state<HTMLButtonElement>();
  const flow = new SpaceFlow((restore) => {
    if (restore) chip?.focus();
  });

  $effect(() => {
    const command = ui.uiCommand();
    if (command.id !== "space.new" && command.id !== "space.edit") return;
    untrack(() => {
      if (requests.claim(command.seq)) requests.ask(command.id === "space.new" ? "new" : "edit");
    });
  });

  // Closing on the press, as every panel in the column does: a press anywhere
  // but the panel and the chip that toggles it, the rest of the bar included.
  $effect(() => {
    if (!flow.open) return;
    const outside = (event: PointerEvent) => {
      const target = event.target;
      if (!(target instanceof Node) || !(panel?.contains(target) || chip?.contains(target)))
        flow.dismiss();
    };
    const away = () => flow.dismiss();
    // WebKit does not focus a pressed button, so Escape is heard window-wide.
    const escape = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || event.isComposing) return;
      event.stopPropagation();
      flow.escape();
    };
    window.addEventListener("pointerdown", outside, true);
    window.addEventListener("blur", away);
    window.addEventListener("keydown", escape);
    return () => {
      window.removeEventListener("pointerdown", outside, true);
      window.removeEventListener("blur", away);
      window.removeEventListener("keydown", escape);
    };
  });
</script>

<!--
  The head of the tab list names the space it holds: its name, with its mark
  when it has one, opens the switcher, and the plus opens a tab in it.
-->
<div class="space-bar" data-cascade data-under-panel style:--cascade={0} role="presentation">
  <button
    bind:this={chip}
    type="button"
    class="space-chip"
    aria-haspopup="dialog"
    aria-expanded={flow.open}
    aria-label={m.space_open_switcher({ name: flow.active?.name ?? "" })}
    data-state={flow.open ? "open" : "closed"}
    onpointerenter={load}
    onfocus={load}
    onclick={() => (flow.open ? flow.dismiss() : flow.show())}
  >
    {#if flow.active}<SpaceGlyph icon={flow.active.icon} color={flow.active.color} />{/if}
    <span class="space-name">{flow.active?.name ?? ""}</span>
    <span class="space-chevron"><Icon icon={ArrowDown01Icon} size={12} strokeWidth={2} /></span>
  </button>
  <div class="space-end">
    {@render extras?.()}
    <button
      type="button"
      class="space-new-tab"
      title={m.new_tab()}
      aria-label={m.new_tab()}
      onclick={tabs.open}><Icon icon={Add01Icon} size={16} /></button
    >
  </div>
  <div
    bind:this={panel}
    class="ui-menu ui-disclosure-panel ui-popover space-panel sidebar-softens"
    role="dialog"
    aria-label={flow.editing ? m.space_edit_label({ name: flow.editing.name }) : m.space_switcher()}
    data-open={flow.open}
    data-editing={flow.editing !== null || undefined}
    inert={!flow.open}
  >
    {#if loaded}<div class="space-panel-body"><SpaceFlowBody {flow} /></div>{/if}
  </div>
</div>

<style>
  .space-bar {
    position: relative;
    z-index: 30;
    display: flex;
    flex: none;
    align-items: center;
    gap: 4px;
    padding-inline: var(--sidebar-inset);
    padding-block: 2px calc(var(--sidebar-row-gap) + 2px);
  }

  .space-chip {
    display: flex;
    flex: 0 1 auto;
    align-items: center;
    gap: 8px;
    min-width: 0;
    height: 30px;
    padding-inline: 10px 7px;
    border: 0;
    border-radius: var(--radius-row);
    background: transparent;
    color: var(--color-label-secondary);
    font: inherit;
    font-size: var(--sidebar-row-text);
    font-weight: 600;
    letter-spacing: -0.01em;
    cursor: default;
    outline: none;
    transition:
      background-color var(--motion-fast) var(--ease-out),
      color var(--motion-fast) var(--ease-out);
  }

  .space-chip:hover,
  .space-chip:focus-visible,
  .space-chip[data-state="open"] {
    background: var(--row-hover);
    color: var(--color-text);
  }

  .space-chip:focus-visible {
    box-shadow: var(--row-rim);
  }

  .space-name {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .space-chevron {
    display: grid;
    flex: none;
    margin-inline-start: -3px;
    color: var(--color-faint);
    place-items: center;
    transition: rotate var(--motion-base) var(--ease-out);
  }

  .space-chip[data-state="open"] .space-chevron {
    rotate: 180deg;
  }

  .space-end {
    display: flex;
    flex: none;
    align-items: center;
    gap: 2px;
    margin-inline-start: auto;
  }

  .space-new-tab {
    display: grid;
    flex: none;
    width: 30px;
    height: 30px;
    padding: 0;
    border: 0;
    border-radius: 50%;
    background: transparent;
    color: var(--color-muted);
    cursor: default;
    place-items: center;
    transition:
      background-color var(--motion-fast) var(--ease-out),
      color var(--motion-fast) var(--ease-out),
      scale var(--motion-slow) var(--ease-spring);
  }

  .space-new-tab:hover,
  .space-new-tab:focus-visible {
    outline: none;
    background: var(--row-hover);
    color: var(--color-text);
  }

  .space-new-tab:active {
    scale: 0.92;
  }

  /* Opens out of the chip it hangs from. */
  .space-panel {
    inset-inline: var(--sidebar-inset);
    min-width: 0;
    padding: 0;
    transform-origin: 44px -10px;

    /* It lies over the list's text: solid, so nothing under it reads. */
    background: var(--color-float);
    backdrop-filter: none;
  }

  /* The list scrolls inside the still material when it runs long; the
     editor keeps its own actions in view and scrolls only its body. */
  .space-panel-body {
    display: flex;
    flex-direction: column;
    max-height: min(560px, calc(100vh - 200px));
    overflow-y: auto;
    overscroll-behavior: contain;
    padding: var(--menu-pad);
  }

  .space-panel[data-editing] .space-panel-body {
    overflow: hidden;
    padding: 0;
  }

  @media (prefers-reduced-motion: reduce) {
    .space-chevron {
      transition: none;
    }
  }
</style>
