<script lang="ts">
  import * as m from "$shared/i18n/messages";
  import { Briefcase02Icon } from "@hugeicons/core-free-icons";
  import { surface } from "$domain/surface";
  import { preferences } from "$domain/preferences";
  import Icon from "$shared/ui/Icon";

  let { compact = false }: { compact?: boolean } = $props();
  let inWork = $derived(surface.currentPage() === "work");
</script>

<!-- With Home in the column's head, Work is a place beside the account. The
     press reads the surface native confirmed, never the request. -->
{#if preferences.enabled("work")}<button
    type="button"
    class="work"
    data-compact={compact}
    aria-pressed={inWork}
    aria-label={m.home_work_entry()}
    title={m.home_work_entry()}
    onclick={() => void surface.open(inWork ? null : "work")}
    ><Icon icon={Briefcase02Icon} size={compact ? 16 : 17} strokeWidth={1.7} /></button
  >{/if}

<style>
  .work {
    display: grid;
    flex: none;
    place-items: center;
    width: 40px;
    height: 40px;
    margin-inline: 2px;
    border: 0;
    border-radius: var(--radius-capsule);
    background: var(--row-active);
    box-shadow: var(--shine-top);
    color: var(--color-label-secondary);
    cursor: default;
    transition:
      background-color var(--motion-fast) var(--ease-out),
      color var(--motion-fast) var(--ease-out);
  }

  .work:hover,
  .work[aria-pressed="true"] {
    background: var(--row-pressed);
    color: var(--color-text);
  }

  .work:active {
    scale: 0.94;
    transition-duration: var(--motion-instant);
  }

  .work[data-compact="true"] {
    width: 36px;
    height: 36px;
    margin-inline: 0;
  }

  @media (prefers-reduced-motion: reduce) {
    .work {
      transition: none;
    }
  }

  :global(:root[data-reduce-motion="true"]) .work {
    transition: none;
  }
</style>
