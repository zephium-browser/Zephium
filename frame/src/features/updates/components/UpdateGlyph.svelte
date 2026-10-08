<!--
  At rail width there is room for one glyph. The waiting update comes first;
  a notice hands its words to Settings › About, which keeps them.
-->
<script lang="ts">
  import Icon from "$shared/ui/Icon";
  import * as notices from "../lib/notices.svelte";
  import { PILL_ICON, activatePill, cardView, pillLabel } from "../lib/present";

  let { onabout }: { onabout: () => void } = $props();

  let selected = $derived(notices.current());
  let card = $derived(selected.card ? cardView(selected.card) : null);
  let pill = $derived(selected.pill);
  let label = $derived(pill ? pillLabel(pill) : (card?.title ?? ""));

  function open() {
    if (pill) {
      activatePill(pill);
      return;
    }
    card?.dismiss();
    onabout();
  }
</script>

{#if pill || card}
  <button
    type="button"
    class="update-plate"
    data-kind={pill ? "pill" : "card"}
    title={label}
    aria-label={label}
    disabled={pill?.kind === "installing"}
    aria-busy={pill?.kind === "installing" || undefined}
    onclick={open}
  >
    {#if pill?.kind === "installing"}<span class="orbit" aria-hidden="true"></span>
    {:else}<Icon icon={pill ? PILL_ICON : card!.icon} size={16} />{/if}
  </button>
{/if}

<style>
  .update-plate {
    display: grid;
    flex: none;
    place-items: center;
    align-self: center;
    width: 40px;
    height: var(--row-sidebar);
    margin-block-start: 6px;
    border: 0;
    border-radius: var(--radius-row);
    background: var(--row-active);
    box-shadow: var(--row-rim);
    color: var(--color-muted);
    cursor: default;
    animation: update-plate-in var(--motion-base) var(--ease-emphasized) both;
    transition:
      background-color var(--motion-fast) var(--ease-out),
      color var(--motion-fast) var(--ease-out),
      scale var(--motion-slow) var(--ease-spring);
  }

  .update-plate:focus-visible {
    outline: 2px solid var(--color-ring);
    outline-offset: 2px;
  }

  .update-plate:hover:not(:disabled) {
    background: var(--row-pressed);
    color: var(--color-text);
  }

  .update-plate[data-kind="pill"] {
    background: var(--color-lit-soft);
    box-shadow: var(--shadow-raise);
    color: var(--color-text);
  }

  .update-plate[data-kind="pill"]:hover:not(:disabled) {
    background: color-mix(in srgb, var(--color-lit) 22%, transparent);
  }

  .update-plate:active:not(:disabled) {
    scale: 0.94;
    transition-duration: var(--motion-instant);
  }

  .orbit {
    width: 14px;
    height: 14px;
    border-radius: var(--radius-capsule);
    background: conic-gradient(from 0deg, transparent 20%, currentcolor);
    mask: radial-gradient(farthest-side, transparent calc(100% - 2px), black calc(100% - 1.5px));
    animation: update-plate-orbit 0.9s linear infinite;
  }

  @keyframes update-plate-in {
    from {
      opacity: 0;
      translate: 0 6px;
    }
  }

  @keyframes update-plate-orbit {
    to {
      rotate: 1turn;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .update-plate {
      animation: none;
    }

    .orbit {
      animation: none;
      opacity: 0.6;
    }
  }

  @media (forced-colors: active) {
    .update-plate {
      border: 1px solid ButtonText;
      box-shadow: none;
    }
  }
</style>
