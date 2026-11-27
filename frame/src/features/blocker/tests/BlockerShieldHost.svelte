<script lang="ts">
  import type { Snippet } from "svelte";
  import BlockerShield from "../components/BlockerShield.svelte";

  let { open, headed = true }: { open: boolean; headed?: boolean } = $props();
</script>

{#snippet head(name: string | null, control?: Snippet)}
  <div class="head">
    <span class="mark"></span>{#if name}<span class="name">{name}</span>{/if}{@render control?.()}
  </div>
{/snippet}

<BlockerShield {open} head={headed ? head : undefined} />
