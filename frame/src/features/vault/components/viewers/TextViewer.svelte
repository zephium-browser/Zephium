<!-- Plain text files, shown as they are. -->
<script lang="ts">
  import type { VaultSession } from "$domain/vault";
  import * as m from "$shared/i18n/messages";
  import NativeBar from "./NativeBar.svelte";

  let { session, path }: { session: VaultSession; path: string } = $props();
  let text = $derived(session.readText(path));
</script>

<div class="text">
  <NativeBar {session} {path} />
  {#await text}
    <p>{m.surface_loading()}</p>
  {:then result}
    {#if result.ok}<pre>{result.value.text}</pre>{:else}<p role="alert">
        {m.vault_open_failed()}
      </p>{/if}
  {/await}
</div>

<style>
  .text {
    display: flex;
    flex-direction: column;
    block-size: 100%;
  }

  pre {
    flex: 1;
    margin: 0;
    padding: 16px 20px;
    overflow: auto;
    font-family: var(--font-mono);
    font-size: 12px;
    white-space: pre-wrap;
  }

  p {
    padding: 16px;
    color: var(--color-muted);
  }
</style>
