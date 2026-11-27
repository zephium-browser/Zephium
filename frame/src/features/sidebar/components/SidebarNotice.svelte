<script lang="ts">
  import {
    Delete02Icon,
    Download01Icon,
    HourglassIcon,
    Link04Icon,
  } from "@hugeicons/core-free-icons";
  import * as attention from "$session/attention.svelte";
  import * as notices from "$session/notice.svelte";
  import Icon from "$shared/ui/Icon";

  /** Long enough to read two words, short enough to never be in the way. */
  const LINGER = 1800;
  /** Matches the exit transition below. */
  const EXIT = 140;

  const GLYPHS = {
    link: Link04Icon,
    download: Download01Icon,
    focus: HourglassIcon,
    data: Delete02Icon,
  };

  let current = $derived(notices.notice());
  let leaving = $state(false);

  $effect(() => {
    if (current?.kind === "focus" || current?.kind === "download") return attention.hold();
  });

  $effect(() => {
    const shown = current;
    if (!shown) return;
    leaving = false;
    const exit = setTimeout(() => (leaving = true), LINGER);
    const gone = setTimeout(() => notices.dismiss(shown.id), LINGER + EXIT);
    return () => {
      clearTimeout(exit);
      clearTimeout(gone);
    };
  });
</script>

{#if current}
  {#key current.id}
    <div class="notice" class:leaving role="status">
      <span class="glyph" aria-hidden="true"><Icon icon={GLYPHS[current.kind]} size={15} /></span>
      <span class="text">{current.text}</span>
    </div>
  {/key}
{/if}

<style>
  .notice {
    display: flex;
    flex: none;
    align-items: center;
    gap: 10px;
    margin: 6px var(--sidebar-inset);
    padding: 8px 12px 8px 8px;
    border-radius: var(--radius-row);
    background: var(--row-active);
    box-shadow: var(--row-rim);
    animation: rise var(--motion-base) var(--ease-emphasized) both;
  }

  .notice.leaving {
    opacity: 0;
    translate: 0 4px;
    transition:
      opacity var(--motion-fast) var(--ease-exit),
      translate var(--motion-fast) var(--ease-exit);
  }

  .glyph {
    display: grid;
    flex: none;
    place-items: center;
    width: 30px;
    height: 30px;
    border-radius: var(--radius-inset);
    background: var(--color-fill);
    color: var(--color-muted);
  }

  .text {
    overflow: hidden;
    color: var(--color-text);
    font-size: var(--text-label);
    font-weight: 500;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  @keyframes rise {
    from {
      opacity: 0;
      translate: 0 6px;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .notice {
      animation: none;
    }

    .notice.leaving {
      transition: none;
    }
  }
</style>
