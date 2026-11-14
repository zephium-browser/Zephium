<script lang="ts">
  import "../Shine/shine.css";
  import "./control.css";
  import type { Snippet } from "svelte";
  import type { HTMLButtonAttributes } from "svelte/elements";
  let {
    children,
    variant = "secondary",
    size = "regular",
    shape = "rounded",
    pending = false,
    disabled = false,
    ref = $bindable(),
    class: className = "",
    ...rest
  }: HTMLButtonAttributes & {
    children: Snippet;
    ref?: HTMLButtonElement;
    /** Primary is the accent fill; secondary is the shine plate; ghost and
     *  danger are quiet and red. */
    variant?: "primary" | "secondary" | "ghost" | "danger";
    /** Compact and regular are the one control size. Large is for a page's
     *  single call to action, such as onboarding. */
    size?: "compact" | "regular" | "large";
    /** Capsule applies to large only. */
    shape?: "rounded" | "capsule";
    pending?: boolean;
  } = $props();
</script>

<button
  bind:this={ref}
  type="button"
  {...rest}
  disabled={disabled || pending}
  aria-busy={pending || undefined}
  class={["ui-button ui-control", variant === "secondary" && "ui-shine ui-shine-press", className]}
  data-variant={variant}
  data-size={size === "large" ? "large" : "regular"}
  data-shape={shape}
>
  {#if pending}<span class="pending" aria-hidden="true"><i></i><i></i><i></i></span>{/if}
  {@render children()}
</button>

<style>
  .ui-button {
    transition:
      background-color var(--motion-instant) var(--ease-smooth),
      color var(--motion-instant) var(--ease-smooth),
      scale var(--motion-fast) var(--ease-smooth),
      opacity var(--motion-fast) var(--ease-smooth);
  }

  .ui-button:focus-visible {
    outline: 2px solid var(--color-ring);
    outline-offset: 2px;
  }

  .ui-button:disabled {
    opacity: 0.5;
  }

  .ui-button:active:not(:disabled) {
    scale: 0.97;
    transition-duration: var(--motion-fast);
  }

  /* Primary is the lit rung, solid: the brightest thing on the row. */
  .ui-button[data-variant="primary"] {
    background: var(--color-lit);
    color: var(--color-on-lit);
  }

  .ui-button[data-variant="primary"]:hover:not(:disabled) {
    background: var(--color-lit-hover);
  }

  .ui-button[data-variant="primary"]:active:not(:disabled) {
    background: var(--color-lit);
  }

  .ui-button[data-variant="ghost"] {
    background: transparent;
    color: var(--color-muted);
  }

  .ui-button[data-variant="ghost"]:hover:not(:disabled) {
    background: var(--color-fill-hover);
    color: var(--color-text);
  }

  .ui-button[data-variant="ghost"]:active:not(:disabled) {
    background: var(--color-fill-pressed);
  }

  .ui-button[data-variant="danger"] {
    background: color-mix(in srgb, var(--color-danger) 14%, transparent);
    color: var(--color-danger);
  }

  .ui-button[data-variant="danger"]:hover:not(:disabled) {
    background: color-mix(in srgb, var(--color-danger) 18%, transparent);
  }

  .ui-button[data-variant="danger"]:active:not(:disabled) {
    background: color-mix(in srgb, var(--color-danger) 24%, transparent);
  }

  .ui-button[data-size="large"] {
    min-height: var(--control-large);
    padding-inline: 16px;
    border-radius: var(--radius-control-large);
    font-size: var(--text-body);
  }

  .ui-button[data-size="large"][data-shape="capsule"] {
    border-radius: var(--radius-capsule);
    padding-inline: 18px;
  }

  .pending {
    display: inline-flex;
    gap: 3px;
  }

  .pending > i {
    width: 4px;
    height: 4px;
    border-radius: 50%;
    background: currentcolor;
    animation: pending 900ms var(--ease-in-out) infinite;
  }

  .pending > i:nth-child(2) {
    animation-delay: 150ms;
  }

  .pending > i:nth-child(3) {
    animation-delay: 300ms;
  }

  @keyframes pending {
    0%,
    100% {
      opacity: 0.25;
    }

    50% {
      opacity: 1;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .ui-button:active:not(:disabled) {
      scale: 1;
    }
  }

  @media (forced-colors: active) {
    /* Opted out of forcing, or the system's text backplate hides the label. */
    .ui-button[data-variant="primary"] {
      forced-color-adjust: none;
      background: Highlight;
      color: HighlightText;
    }
  }
</style>
