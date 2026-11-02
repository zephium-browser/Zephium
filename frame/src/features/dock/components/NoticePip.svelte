<!--
  A collapsed sidebar has no room for a sentence, so a notice becomes its
  glyph alone, popped onto the corner of the tool case for a moment. The
  words stay available to assistive technology.
-->
<script lang="ts">
  import {
    Delete02Icon,
    Download01Icon,
    HourglassIcon,
    Link04Icon,
  } from "@hugeicons/core-free-icons";
  import * as notices from "$session/notice.svelte";
  import Icon from "$shared/ui/Icon";

  const GLYPHS = {
    link: Link04Icon,
    download: Download01Icon,
    focus: HourglassIcon,
    data: Delete02Icon,
  };
  /** A glyph reads faster than a sentence, so it leaves sooner. */
  const LINGER = 1600;
  /** Matches the exit transition below. */
  const EXIT = 160;

  let current = $derived(notices.notice());
  let leaving = $state(false);

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
    <span class="pip" class:leaving role="status">
      <Icon icon={GLYPHS[current.kind]} size={12} />
      <span class="words">{current.text}</span>
    </span>
  {/key}
{/if}

<style>
  .pip {
    position: absolute;
    inset-block-start: -6px;
    inset-inline-end: -6px;
    z-index: 2;
    display: grid;
    place-items: center;
    inline-size: 22px;
    block-size: 22px;
    border-radius: var(--radius-capsule);
    background: var(--color-lit);
    box-shadow:
      0 0 0 2px var(--color-chrome),
      var(--shadow-float);
    color: var(--color-on-lit);
    pointer-events: none;
    animation: pop var(--motion-slow) var(--ease-spring) both;
  }

  .pip.leaving {
    opacity: 0;
    scale: 0.6;
    transition:
      opacity var(--motion-fast) var(--ease-exit),
      scale var(--motion-fast) var(--ease-exit);
  }

  .words {
    position: absolute;
    inline-size: 1px;
    block-size: 1px;
    overflow: hidden;
    clip-path: inset(50%);
    white-space: nowrap;
  }

  @keyframes pop {
    from {
      opacity: 0;
      scale: 0.4;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .pip {
      animation: none;
    }

    .pip.leaving {
      transition: none;
    }
  }

  :global(:root[data-reduce-motion="true"]) .pip {
    animation: none;
    transition: none;
  }

  @media (forced-colors: active) {
    .pip {
      border: 1px solid CanvasText;
    }
  }
</style>
