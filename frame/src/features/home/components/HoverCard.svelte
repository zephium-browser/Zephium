<script lang="ts">
  import type { Row } from "../lib/model";
  let { row, x, y }: { row: Row; x: number; y: number } = $props();
</script>

<div class="hover" role="tooltip" style:left={`${x}px`} style:top={`${y}px`}>
  <p class="title" dir="auto">{row.label}</p>
  {#if row.host}<p class="host" dir="auto">{row.host}</p>{/if}
  {#if row.note}<p class="note" dir="auto">{row.note}</p>{/if}
  {#if row.known.length}
    <ul>
      {#each row.known as section (section.id)}<li dir="auto">{section.label}</li>{/each}
    </ul>
  {/if}
</div>

<style>
  /* Only what is already held about the row: nothing here asks for more. */
  .hover {
    position: absolute;
    z-index: 4;
    box-sizing: border-box;
    inline-size: 260px;
    padding: 12px 14px;
    border-radius: var(--radius-card);
    background: var(--color-raised);
    box-shadow: var(--shadow-popover);
    color: var(--color-text);
    pointer-events: none;
    animation: show var(--motion-fast) var(--ease-out);
  }

  p {
    margin: 0;
  }

  .title {
    overflow: hidden;
    font-size: var(--text-body);
    font-weight: 600;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .host {
    color: var(--color-faint);
    font-size: var(--text-label);
  }

  .note {
    display: -webkit-box;
    margin-block-start: 4px;
    overflow: hidden;
    color: var(--color-muted);
    font-size: var(--text-label);
    line-height: 17px;
    -webkit-box-orient: vertical;
    -webkit-line-clamp: 3;
    line-clamp: 3;
  }

  ul {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    margin: 10px 0 0;
    padding: 0;
    list-style: none;
  }

  li {
    max-inline-size: 100%;
    padding: 2px 8px;
    overflow: hidden;
    border-radius: var(--radius-capsule);
    background: var(--color-fill);
    color: var(--color-label-secondary);
    font-size: var(--text-caption);
    line-height: 16px;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  @keyframes show {
    from {
      opacity: 0;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .hover {
      animation: none;
    }
  }

  :global(:root[data-reduce-motion="true"]) .hover {
    animation: none;
  }
</style>
