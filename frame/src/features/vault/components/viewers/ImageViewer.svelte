<!-- An image in the vault, fitted to the pane; click to see it at full size. -->
<script lang="ts">
  import type { VaultSession } from "$domain/vault";
  import { nameOf } from "$domain/vault";

  let { session, path }: { session: VaultSession; path: string } = $props();
  let full = $state(false);
</script>

<div class="image" class:full>
  <button type="button" onclick={() => (full = !full)}>
    <img src={session.url(path)} alt={nameOf(path)} />
  </button>
</div>

<style>
  .image {
    display: grid;
    place-items: center;
    block-size: 100%;
    overflow: auto;
    background: var(--color-canvas);
  }

  button {
    padding: 0;
    border: 0;
    background: none;
    cursor: zoom-in;
  }

  img {
    display: block;
    max-inline-size: 100%;
    max-block-size: 100%;
  }

  .full button {
    cursor: zoom-out;
  }

  .full img {
    max-inline-size: none;
    max-block-size: none;
  }
</style>
