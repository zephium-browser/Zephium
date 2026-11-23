<script lang="ts">
  import type { Snippet } from "svelte";
  let {
    width,
    height,
    enter = false,
    label,
    children,
  }: {
    width: number;
    height: number;
    enter?: boolean;
    label?: string;
    children: Snippet;
  } = $props();
</script>

<section
  class="card"
  class:enter
  aria-label={label}
  style:inline-size={`${width}px`}
  style:block-size={`${height}px`}
>
  {@render children()}
</section>

<style>
  /* Solid, a step above the canvas: it lifts by its own tone, with only a
     contact shadow under it in the light theme. */
  .card {
    position: relative;
    box-sizing: border-box;
    display: flex;
    flex-direction: column;
    padding: var(--home-pad);
    overflow: hidden;
    border-radius: var(--radius-card);
    background: var(--color-surface);
    box-shadow: var(--shadow-card);
    color: var(--color-text);
    contain: layout paint;
  }

  .enter {
    animation: card-in var(--motion-page) var(--ease-emphasized) backwards;
  }

  @keyframes card-in {
    from {
      opacity: 0;
      translate: 0 6px;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .enter {
      animation: none;
    }
  }

  :global(:root[data-reduce-motion="true"]) .enter {
    animation: none;
  }
</style>
