<script lang="ts">
  import { layout, type GlanceView } from "$domain/layout";
  import { tabs } from "$domain/tabs";
  import * as m from "$shared/i18n/messages";
  import Icon from "$shared/ui/Icon";
  import {
    ArrowExpand01Icon,
    Cancel01Icon,
    LayoutTwoColumnIcon,
    Link01Icon,
    Tick02Icon,
  } from "../lib/icons";

  let { view }: { view: GlanceView } = $props();

  let shownBackdrop = $state<string | null>(null);
  let copied = $state(false);
  let copiedTimer: ReturnType<typeof setTimeout> | undefined;

  const settled = $derived(!!view.backdrop && shownBackdrop === view.backdrop);
  const tools = $derived({
    left: view.page.x + view.page.width - view.region.x,
    top: view.page.y - view.region.y,
  });

  function keydown(event: KeyboardEvent) {
    if (event.key !== "Escape" || event.defaultPrevented) return;
    event.preventDefault();
    layout.glanceAct("close");
  }

  async function copy() {
    clearTimeout(copiedTimer);
    copied = await tabs.copyActiveLink();
    if (copied) copiedTimer = setTimeout(() => (copied = false), 1400);
  }

  $effect(() => () => clearTimeout(copiedTimer));
</script>

<svelte:window onkeydown={keydown} />

<div
  class="glance"
  data-glance
  style:left="{view.region.x}px"
  style:top="{view.region.y}px"
  style:width="{view.region.width}px"
  style:height="{view.region.height}px"
>
  <button
    type="button"
    class="backdrop"
    aria-label={m.glance_close()}
    onclick={() => layout.glanceAct("close")}
  >
    {#if view.backdrop}<img
        src={view.backdrop}
        alt=""
        draggable="false"
        class:settled
        onload={() => (shownBackdrop = view.backdrop)}
      />{/if}
  </button>
  <div class="tools" style:left="{tools.left}px" style:top="{tools.top}px">
    <button
      type="button"
      class="tool"
      aria-label={m.glance_close()}
      title={m.glance_close()}
      onclick={() => layout.glanceAct("close")}
    >
      <Icon icon={Cancel01Icon} size={17} strokeWidth={1.8} />
    </button>
    <button
      type="button"
      class="tool"
      aria-label={m.glance_expand()}
      title={m.glance_expand()}
      onclick={() => layout.glanceAct("expand")}
    >
      <Icon icon={ArrowExpand01Icon} size={17} strokeWidth={1.8} />
    </button>
    <button
      type="button"
      class="tool"
      aria-label={m.glance_split()}
      title={m.glance_split()}
      onclick={() => layout.glanceAct("split")}
    >
      <Icon icon={LayoutTwoColumnIcon} size={17} strokeWidth={1.8} />
    </button>
    <button
      type="button"
      class="tool"
      aria-label={copied ? m.glance_copied() : m.glance_copy()}
      title={copied ? m.glance_copied() : m.glance_copy()}
      onclick={() => void copy()}
    >
      <Icon icon={copied ? Tick02Icon : Link01Icon} size={17} strokeWidth={1.8} />
    </button>
  </div>
</div>

<style>
  .glance {
    position: fixed;
    z-index: 30;
  }

  .backdrop {
    position: absolute;
    inset: 0;
    padding: 0;
    border: 0;
    background: transparent;
    cursor: default;
  }

  .backdrop img {
    width: 100%;
    height: 100%;
    border-radius: var(--radius-control);
    object-fit: cover;
    pointer-events: none;
    user-select: none;
    transform-origin: center;
  }

  .backdrop img.settled {
    opacity: 0.3;
    scale: 0.97;
    transition:
      opacity 350ms var(--ease-spring),
      scale 350ms var(--ease-spring);
  }

  .tools {
    position: absolute;
    display: flex;
    flex-direction: column;
    gap: 12px;
    padding: 12px;
    animation: tools-in var(--motion-base) var(--ease-out) backwards;
  }

  .tool {
    display: grid;
    place-items: center;
    width: 36px;
    height: 36px;
    padding: 0;
    border: 0;
    border-radius: var(--radius-capsule);
    background: var(--color-float);
    box-shadow: var(--shadow-menu), var(--shine-top);
    color: var(--color-text);
    transition:
      background-color 50ms ease,
      scale 50ms ease;
  }

  .tool:focus-visible {
    outline: 2px solid var(--color-ring);
    outline-offset: 2px;
  }

  .tool:hover {
    background: color-mix(in srgb, var(--color-float) 90%, var(--color-text));
    scale: 1.02;
  }

  .tool:hover:active {
    scale: 0.98;
  }

  @keyframes tools-in {
    from {
      opacity: 0;
      translate: -6px 0;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .backdrop img.settled,
    .tool {
      transition: none;
    }

    .tools {
      animation: none;
    }
  }
</style>
