<script lang="ts" module>
  import type { IconSvgElement } from "@hugeicons/svelte";

  export type SidebarCardAction = {
    label: string;
    icon: IconSvgElement;
    onclick: () => void;
    /** The card leaves once the action has run. */
    dismisses?: boolean;
  };
</script>

<script lang="ts">
  import { Cancel01Icon } from "@hugeicons/core-free-icons";
  import Icon from "../Icon/Icon.svelte";
  import IconButton from "../IconButton/IconButton.svelte";
  import { duration, reducedMotion } from "$shared/lib/motion";

  let {
    variant = "card",
    title,
    detail,
    items = [],
    icon,
    pending = false,
    onclick,
    dismissLabel = "",
    ondismiss,
    actions = [],
  }: {
    /** A pill is one action, the whole of it clickable; a card is a notice
     *  that can be dismissed, with the actions it offers as rows. */
    variant?: "pill" | "card";
    title: string;
    detail?: string;
    /** A short list under the detail, such as a release's highlights. */
    items?: string[];
    /** The pill's glyph, before its label. */
    icon?: IconSvgElement;
    pending?: boolean;
    onclick?: () => void;
    dismissLabel?: string;
    ondismiss?: () => void;
    actions?: SidebarCardAction[];
  } = $props();

  const uid = $props.id();
  let leaving = $state(false);

  function leave() {
    if (leaving) return;
    leaving = true;
    const done = ondismiss;
    setTimeout(() => done?.(), reducedMotion() ? 0 : duration("fast"));
  }

  function act(action: SidebarCardAction) {
    action.onclick();
    if (action.dismisses) leave();
  }
</script>

{#if variant === "pill"}
  <button
    type="button"
    class="sidebar-card"
    data-variant="pill"
    disabled={pending}
    aria-busy={pending || undefined}
    {onclick}
  >
    {#if pending}<span class="orbit" aria-hidden="true"></span>{:else if icon}<Icon
        {icon}
        size={14}
      />{/if}
    <span class="pill-label">{title}</span>
  </button>
{:else}
  <div class="sidebar-card" data-variant="card" class:leaving role="group" aria-labelledby={uid}>
    <div class="head">
      <div class="copy">
        <strong id={uid}>{title}</strong>
        {#if detail}<p>{detail}</p>{/if}
        {#if items.length > 0}<ul class="items">
            {#each items as item, index (index)}<li>{item}</li>{/each}
          </ul>{/if}
      </div>
      {#if ondismiss}<IconButton
          icon={Cancel01Icon}
          label={dismissLabel}
          size={13}
          buttonSize={24}
          onclick={leave}
        />{/if}
    </div>
    {#if actions.length > 0}
      <div class="actions">
        {#each actions as action (action.label)}
          <button type="button" class="action" onclick={() => act(action)}>
            <Icon icon={action.icon} size={15} />
            <span>{action.label}</span>
          </button>
        {/each}
      </div>
    {/if}
  </div>
{/if}

<style>
  /* The same plate as every other passing thing at the foot of the column,
     rounded as a card rather than a row because it holds rows of its own. */
  .sidebar-card {
    flex: none;
    box-sizing: border-box;
    animation: sidebar-card-in var(--motion-base) var(--ease-emphasized) both;
  }

  .sidebar-card[data-variant="card"] {
    display: grid;
    gap: 4px;
    padding: 10px 6px 6px;
    border-radius: var(--radius-card);
    background: var(--row-active);
    box-shadow: var(--row-rim);
  }

  /* The title lines up with the glyphs of the rows under it. */
  .head {
    display: flex;
    align-items: flex-start;
    gap: 8px;
    padding-inline-start: 8px;
  }

  .copy {
    display: grid;
    flex: 1;
    gap: 2px;
    min-inline-size: 0;
    padding-block: 4px 2px;
  }

  .items {
    display: grid;
    gap: 3px;
    margin: 4px 0 0;
    padding: 0;
    list-style: none;
    color: var(--color-muted);
    font-size: var(--text-caption);
    line-height: 1.35;
  }

  .items li {
    position: relative;
    padding-inline-start: 10px;
  }

  .items li::before {
    content: "";
    position: absolute;
    inset-block-start: 0.6em;
    inset-inline-start: 1px;
    inline-size: 3px;
    block-size: 3px;
    border-radius: var(--radius-capsule);
    background: currentcolor;
  }

  strong {
    color: var(--color-text);
    font-size: var(--text-label);
    font-weight: 550;
    line-height: 16px;
  }

  p {
    margin: 0;
    color: var(--color-muted);
    font-size: var(--text-caption);
    line-height: 15px;
  }

  .head :global(.icon-button) {
    flex: none;
    color: var(--color-faint);
  }

  .head :global(.icon-button:hover) {
    color: var(--color-text);
  }

  .actions {
    display: grid;
  }

  .action {
    display: flex;
    align-items: center;
    gap: 10px;
    block-size: 30px;
    padding-inline: 8px;
    border: 0;
    border-radius: var(--radius-row);
    background: transparent;
    color: var(--color-text);
    font: inherit;
    font-size: var(--text-label);
    text-align: start;
    cursor: default;
    transition: background-color var(--motion-instant) var(--ease-smooth);
  }

  .action > :global(svg) {
    flex: none;
    color: var(--color-muted);
  }

  .action:hover {
    background: var(--row-hover);
  }

  .action:active {
    background: var(--row-pressed);
  }

  /* A notice the next version brought: lit softly, never the full lit rung,
     which this column keeps for what is on. */
  .sidebar-card[data-variant="pill"] {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 7px;
    inline-size: 100%;
    block-size: 32px;
    padding-inline: 14px;
    border: 0;
    border-radius: var(--radius-capsule);
    background: var(--color-lit-soft);
    box-shadow: var(--shadow-raise);
    color: var(--color-text);
    font: inherit;
    font-size: var(--text-label);
    font-weight: 550;
    cursor: default;
    transition:
      background-color var(--motion-fast) var(--ease-out),
      scale var(--motion-slow) var(--ease-spring);
  }

  .sidebar-card[data-variant="pill"]:disabled {
    color: var(--color-muted);
  }

  .sidebar-card[data-variant="pill"]:hover:not(:disabled) {
    background: color-mix(in srgb, var(--color-lit) 22%, transparent);
  }

  .sidebar-card[data-variant="pill"]:active:not(:disabled) {
    scale: 0.97;
    transition-duration: var(--motion-instant);
  }

  .pill-label {
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  /* The tab favicon's loading arc, at label size. */
  .orbit {
    flex: none;
    inline-size: 12px;
    block-size: 12px;
    border-radius: var(--radius-capsule);
    background: conic-gradient(from 0deg, transparent 20%, currentcolor);
    mask: radial-gradient(farthest-side, transparent calc(100% - 2px), black calc(100% - 1.5px));
    animation: sidebar-card-orbit 0.9s linear infinite;
  }

  .sidebar-card:focus-visible,
  .action:focus-visible {
    outline: 2px solid var(--color-ring);
    outline-offset: 2px;
  }

  .sidebar-card.leaving {
    opacity: 0;
    translate: 0 4px;
    transition:
      opacity var(--motion-fast) var(--ease-exit),
      translate var(--motion-fast) var(--ease-exit);
  }

  @keyframes sidebar-card-in {
    from {
      opacity: 0;
      translate: 0 6px;
    }
  }

  @keyframes sidebar-card-orbit {
    to {
      rotate: 1turn;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .sidebar-card,
    .sidebar-card.leaving {
      animation: none;
      transition: none;
    }

    .orbit {
      animation: none;
      opacity: 0.6;
    }
  }

  @media (forced-colors: active) {
    .sidebar-card[data-variant="card"] {
      border: 1px solid CanvasText;
    }

    .sidebar-card[data-variant="pill"],
    .action {
      border: 1px solid ButtonText;
      box-shadow: none;
    }

    .action {
      border-color: transparent;
    }

    .action:hover,
    .action:focus-visible {
      border-color: Highlight;
    }
  }
</style>
