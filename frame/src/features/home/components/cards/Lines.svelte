<script lang="ts">
  import type { HomeNode } from "../../lib/layout";
  let { data }: { data: { item: Extract<HomeNode, { kind: "lines" }> } } = $props();
  let item = $derived(data.item);
</script>

<svg
  class="lines"
  aria-hidden="true"
  width={item.box.width}
  height={item.box.height}
  viewBox={`0 0 ${item.box.width} ${item.box.height}`}
>
  {#each item.lines as line, index (index)}
    <path d={line.d} class={line.state} />
  {/each}
</svg>

<style>
  .lines {
    display: block;
    overflow: visible;
    pointer-events: none;
  }

  path {
    fill: none;
    stroke: var(--color-border-strong);
    stroke-width: 1.5;
    stroke-linecap: round;
  }

  /* A page being read: the line runs toward where its inside will be. */
  .opening {
    stroke-dasharray: 3 5;
    animation: run 900ms linear infinite;
  }

  .refused {
    stroke-dasharray: 1 4;
  }

  @keyframes run {
    to {
      stroke-dashoffset: -16;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .opening {
      animation: none;
    }
  }

  :global(:root[data-reduce-motion="true"]) .opening {
    animation: none;
  }
</style>
