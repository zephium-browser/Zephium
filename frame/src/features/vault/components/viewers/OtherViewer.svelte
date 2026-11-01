<!-- A file Zephium cannot show: open it in its own app or find it in the folder. -->
<script lang="ts">
  import { onMount } from "svelte";
  import type { VaultEntry, VaultSession } from "$domain/vault";
  import * as m from "$shared/i18n/messages";
  import NativeBar from "./NativeBar.svelte";
  import { formatSize } from "../../lib/format";

  let { session, path }: { session: VaultSession; path: string } = $props();
  let entry = $state<VaultEntry | null>(null);
  onMount(async () => {
    const response = await session.call({ kind: "stat", path });
    if (response.kind === "entry") entry = response.entry;
  });
</script>

<div class="other">
  <NativeBar {session} {path} note={m.vault_no_preview()} />
  <div class="card">
    <strong>{entry?.name ?? path}</strong>
    {#if entry}<span
        >{formatSize(Number(entry.size))} · {new Date(
          Number(entry.modified),
        ).toLocaleString()}</span
      >{/if}
  </div>
</div>

<style>
  .other {
    display: flex;
    flex-direction: column;
    block-size: 100%;
  }

  .card {
    display: grid;
    gap: 6px;
    place-content: center;
    flex: 1;
    color: var(--color-muted);
    text-align: center;
  }

  strong {
    color: var(--color-text);
    font-size: 15px;
  }
</style>
