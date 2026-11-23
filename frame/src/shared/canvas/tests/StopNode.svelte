<script lang="ts">
  import type { Node, NodeProps } from "@xyflow/svelte";
  import { focusStop } from "..";
  let { id, data }: NodeProps<Node<{ rows: string[] }, "stops">> = $props();
  const stop = focusStop();
</script>

<div class="card">
  <button type="button" {@attach stop(id)}>{id}</button>
  {#each data.rows as row (row)}<span role="link" tabindex="-1" aria-label={row} {@attach stop(row)}
      >{row}</span
    >{/each}
</div>

<style>
  .card {
    display: grid;
    inline-size: 240px;
  }

  button,
  span {
    block-size: 32px;
  }
</style>
