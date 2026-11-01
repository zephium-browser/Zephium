<!-- The bar over a view-only file: where it is, and the desktop app that edits it. -->
<script lang="ts">
  import type { VaultSession } from "$domain/vault";
  import * as m from "$shared/i18n/messages";
  import { extOf, nativeEditor } from "../../lib/kinds";

  let {
    session,
    path,
    note = "",
  }: { session: VaultSession; path: string; note?: string } = $props();
  let editor = $derived(nativeEditor(extOf(path)));
  let failed = $state(false);

  async function openNative() {
    const response = await session.call({ kind: "open_native", path });
    failed = response.kind === "error";
  }
</script>

<div class="bar">
  <span class="note">{failed ? m.vault_open_native_failed() : note || m.vault_view_only()}</span>
  <button type="button" data-keytip="R" onclick={() => void session.call({ kind: "reveal", path })}
    >{m.vault_show_in_folder()}</button
  >
  <button type="button" class="primary" data-keytip="E" onclick={() => void openNative()}>
    {editor ? m.vault_edit_in({ app: editor }) : m.vault_open_default()}
  </button>
</div>

<style>
  .bar {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 10px;
    border-block-end: 1px solid var(--color-border);
    font-size: 12px;
  }

  .note {
    flex: 1;
    overflow: hidden;
    color: var(--color-muted);
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  button {
    block-size: 26px;
    padding-inline: 10px;
    border: 0;
    border-radius: var(--radius-control-compact);
    background: transparent;
    color: var(--color-label-secondary);
    font: inherit;
  }

  button:hover {
    background: var(--color-fill-hover);
    color: var(--color-text);
  }

  button.primary {
    background: var(--color-fill-active);
    color: var(--color-text);
  }
</style>
