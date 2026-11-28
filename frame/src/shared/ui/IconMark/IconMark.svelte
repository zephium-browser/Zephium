<script lang="ts">
  import "../space-hue.css";
  import Icon from "$shared/ui/Icon";
  import { SPACE_COLORS, SPACE_SYMBOLS } from "$shared/lib/space-symbols";

  let {
    icon,
    color = 0,
    size = 16,
  }: {
    icon: { kind: "symbol"; index: number } | { kind: "emoji"; text: string } | null;
    color?: number;
    size?: number;
  } = $props();
  const hue = $derived(Number.isInteger(color) && color >= 0 && color < SPACE_COLORS ? color : 0);
  const symbol = $derived(
    icon?.kind === "symbol" ? (SPACE_SYMBOLS[icon.index] ?? SPACE_SYMBOLS[0]) : null,
  );
</script>

<!-- A person's mark: a curated glyph in a hue, or an emoji as it was typed.
     It is never the accessible name; the caller keeps that as page text. -->
{#if icon !== null}<span
    class="space-glyph"
    data-space-hue={hue}
    data-kind={icon.kind}
    style:--glyph-size={`${size}px`}
    aria-hidden="true"
    >{#if icon.kind === "emoji"}<span class="emoji">{icon.text}</span>{:else if symbol}<Icon
        icon={symbol}
        {size}
        strokeWidth={size >= 24 ? 1.6 : 1.8}
      />{/if}</span
  >{/if}

<style>
  .space-glyph {
    display: grid;
    flex: none;
    inline-size: var(--glyph-size);
    block-size: var(--glyph-size);
    color: var(--space-ink);
    place-items: center;
  }

  .emoji {
    font-size: calc(var(--glyph-size) * 0.94);
    line-height: 1;
  }
</style>
