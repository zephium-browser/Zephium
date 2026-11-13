<script lang="ts">
  import { onDestroy } from "svelte";
  import "./space-hue.css";
  import * as m from "$shared/i18n/messages";
  import { Add01Icon, Layers01Icon, PencilEdit02Icon } from "@hugeicons/core-free-icons";
  import type { SpaceView } from "$shared/ipc/bindings";
  import { tabs } from "$domain/tabs";
  import Disclosure from "$shared/ui/Disclosure";
  import Icon from "$shared/ui/Icon";
  import SpaceGlyph from "./SpaceGlyph.svelte";
  import { spaceKeys } from "../lib/space-keys";
  import * as requests from "../lib/space-requests.svelte";

  let { active, roomy = false }: { active: SpaceView; roomy?: boolean } = $props();
  const spaces = $derived(tabs.spaces());

  // Holding Control names the key of every row in place of its mark.
  let ctrl = $state(false);
  const read = (event: KeyboardEvent) => (ctrl = event.ctrlKey);
  const lost = () => (ctrl = false);
  function listen() {
    window.addEventListener("keydown", read, true);
    window.addEventListener("keyup", read, true);
    window.addEventListener("blur", lost);
  }
  function unlisten() {
    window.removeEventListener("keydown", read, true);
    window.removeEventListener("keyup", read, true);
    window.removeEventListener("blur", lost);
    ctrl = false;
  }
  onDestroy(unlisten);
</script>

<!--
  The rail's spaces, as the profile menu is: a short stack under the button.
  Alone at rail width each space is a mark named by its tooltip; with a tool or
  Work beside the rail there is room, and the stack names them.
-->
<Disclosure
  label={m.space_open_switcher({ name: active.name })}
  triggerTitle={active.name}
  menu
  triggerClass="rail-item rail-spaces"
  panelClass={`rail-space-stack sidebar-softens${roomy ? " rail-space-stack-roomy" : ""}`}
  onopen={listen}
  onclose={unlisten}
>
  {#snippet trigger()}{#if active.icon}<SpaceGlyph
        icon={active.icon}
        color={active.color}
        size={18}
      />{:else}<Icon icon={Layers01Icon} size={16} />{/if}{/snippet}
  {#each spaces as space, index (space.id)}
    {@const keys = spaceKeys(index)}
    <button
      type="button"
      role="menuitem"
      class="ui-menu-item rail-space"
      style:--step={index}
      title={keys ? `${space.name}  ${keys}` : space.name}
      aria-label={space.name}
      aria-current={space.id === active.id || undefined}
      aria-keyshortcuts={keys ?? undefined}
      onclick={() => space.id !== active.id && void tabs.switchSpace(space.id)}
    >
      {#if ctrl && keys && !roomy}<span class="rail-space-keys">{keys}</span
        >{:else if space.icon}<SpaceGlyph
          icon={space.icon}
          color={space.color}
          size={18}
        />{:else}<span class="rail-space-initial" data-space-hue={space.color} aria-hidden="true"
          >{[...space.name][0] ?? ""}</span
        >{/if}
      {#if roomy}<span class="rail-space-name">{space.name}</span>{#if keys}<span
            class="rail-space-keys">{keys}</span
          >{/if}{/if}
    </button>
  {/each}
  <div class="rail-space-rule" role="separator"></div>
  <button
    type="button"
    role="menuitem"
    class="ui-menu-item rail-space"
    style:--step={spaces.length}
    title={m.space_new()}
    aria-label={m.space_new()}
    onclick={() => requests.openInColumn("new")}
  >
    <Icon icon={Add01Icon} size={16} strokeWidth={1.8} />
    {#if roomy}<span class="rail-space-name">{m.space_new()}</span>{/if}
  </button>
  <button
    type="button"
    role="menuitem"
    class="ui-menu-item rail-space"
    style:--step={spaces.length + 1}
    title={m.space_edit_label({ name: active.name })}
    aria-label={m.space_edit_label({ name: active.name })}
    onclick={() => requests.openInColumn("edit")}
  >
    <Icon icon={PencilEdit02Icon} size={15} />
    {#if roomy}<span class="rail-space-name">{m.space_edit_label({ name: active.name })}</span>{/if}
  </button>
</Disclosure>

<style>
  /* Centred on the square button, six points either side of it. The rail's
     marks lie behind it, so it is near-solid rather than glass. */
  :global(.ui-disclosure-panel.rail-space-stack[data-align]) {
    top: calc(100% + 4px);
    inset-inline-start: -6px;
    width: 48px;
    min-width: 0;
    max-height: calc(100vh - 140px);
    overflow-y: auto;
    overscroll-behavior: contain;
    background: color-mix(in srgb, var(--color-float) 96%, transparent);
    transform-origin: top center;
  }

  /* Named, the stack is wider than the rail and spills past it, over the
     panel or page beside it. */
  :global(.ui-disclosure-panel.rail-space-stack.rail-space-stack-roomy[data-align]) {
    width: 224px;
    max-width: none;
  }

  :global(.ui-disclosure-panel.rail-space-stack[data-align]:not([data-open="true"])) {
    translate: 0 -6px;
  }

  .rail-space {
    justify-content: center;
    width: 100%;
    min-height: 34px;
    padding: 0;
    border: 0;
    background: transparent;
    color: var(--color-muted);
    font: inherit;
  }

  .rail-space:hover,
  .rail-space:focus-visible {
    background: var(--row-active);
    color: var(--color-text);
  }

  .rail-space:active {
    background: var(--row-pressed);
  }

  .rail-space[aria-current="true"] {
    background: var(--row-active);
    box-shadow: var(--shine-top);
    color: var(--color-text);
  }

  :global(.rail-space-stack-roomy) .rail-space {
    justify-content: flex-start;
    gap: 10px;
    padding-inline: 9px;
  }

  .rail-space-name {
    flex: 1;
    overflow: hidden;
    color: var(--color-text);
    font-size: var(--text-body);
    text-align: start;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .rail-space-initial {
    display: grid;
    place-items: center;
    inline-size: 18px;
    block-size: 18px;
    color: var(--space-ink);
    font-size: var(--text-body);
    font-weight: 600;
    text-transform: uppercase;
  }

  .rail-space-keys {
    color: var(--color-muted);
    font-size: var(--text-label);
    font-variant-numeric: tabular-nums;
    letter-spacing: 0.02em;
  }

  .rail-space-rule {
    height: 1px;
    margin: 4px var(--menu-item-inset);
    background: var(--color-border);
  }

  :global(.rail-space-stack[data-open="true"]) .rail-space {
    animation: rail-space-in var(--motion-slow) var(--ease-out) both;
    animation-delay: calc(var(--step) * 14ms);
  }

  @keyframes rail-space-in {
    from {
      opacity: 0;
      translate: 0 -6px;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    :global(.rail-space-stack[data-open="true"]) .rail-space {
      animation: none;
    }
  }
</style>
