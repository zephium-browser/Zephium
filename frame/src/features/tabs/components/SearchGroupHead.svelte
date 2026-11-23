<script lang="ts">
  import * as m from "$shared/i18n/messages";
  import { commands } from "$shared/ipc/bindings";
  import { settle } from "$domain/operations";

  let {
    search,
    text,
    motionKey,
    class: className = "",
  }: { search: string; text: string; motionKey?: string; class?: string } = $props();
</script>

<!-- What the pages under it were found by. It takes nothing dropped on it and
     folds nothing; it shows Home at that search, which native settles. -->
<li class={["search-head", className]} data-motion-key={motionKey}>
  <button
    type="button"
    aria-label={m.home_search_group({ text })}
    onclick={() => void settle(commands.homeFocusSearch(search))}
    ><span dir="auto">{text}</span></button
  >
</li>

<style>
  .search-head {
    display: flex;
    margin-block-start: 6px;
  }

  .search-head:first-child {
    margin-block-start: 0;
  }

  button {
    display: flex;
    flex: 1;
    align-items: center;
    min-width: 0;
    height: 24px;
    padding-inline: 10px;
    border: 0;
    border-radius: var(--radius-row);
    background: transparent;
    color: var(--color-faint);
    font: inherit;
    font-size: var(--text-caption);
    font-weight: 600;
    text-align: start;
    cursor: default;
    transition: color var(--motion-fast) var(--ease-out);
  }

  button:hover {
    color: var(--color-text);
  }

  button:focus-visible {
    outline-offset: -2px;
  }

  /* The fade sits at the column's edge, so only words that reach it fade. */
  span {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    white-space: nowrap;
    mask-image: var(--mask-fade-end);
  }

  span:dir(rtl) {
    mask-image: var(--mask-fade-end-rtl);
  }

  @media (prefers-reduced-motion: reduce) {
    button {
      transition: none;
    }
  }

  :global(:root[data-reduce-motion="true"]) button {
    transition: none;
  }

  @media (forced-colors: active) {
    span {
      mask-image: none;
    }
  }
</style>
