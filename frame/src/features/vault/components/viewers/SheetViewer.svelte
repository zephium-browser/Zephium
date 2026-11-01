<!-- An Excel workbook or CSV as a read-only grid with its sheets as tabs. Edit it in Excel. -->
<script lang="ts">
  import { onMount } from "svelte";
  import type { VaultSession } from "$domain/vault";
  import * as m from "$shared/i18n/messages";
  import NativeBar from "./NativeBar.svelte";
  import { columnName, readCsv, readWorkbook, type Sheet } from "../../lib/sheet";
  import { extOf } from "../../lib/kinds";

  let { session, path }: { session: VaultSession; path: string } = $props();
  const PAGE = 500;
  let sheets = $state.raw<Sheet[]>([]);
  let current = $state(0);
  let shown = $state(PAGE);
  let status = $state<"loading" | "ready" | "failed">("loading");
  let sheet = $derived(sheets[current]);
  let width = $derived(Math.max(1, ...(sheet?.rows.map((row) => row.length) ?? [1])));

  onMount(() => {
    let alive = true;
    void (async () => {
      try {
        const response = await fetch(session.url(path));
        if (!response.ok) throw new Error(String(response.status));
        const result =
          extOf(path) === "csv"
            ? [readCsv(await response.text())]
            : await readWorkbook(await response.arrayBuffer());
        if (!alive) return;
        sheets = result;
        status = "ready";
      } catch {
        if (alive) status = "failed";
      }
    })();
    return () => {
      alive = false;
    };
  });
</script>

<div class="sheet">
  <NativeBar {session} {path} note={extOf(path) === "xlsm" ? m.vault_macros_desktop() : ""} />
  {#if status === "loading"}
    <p>{m.surface_loading()}</p>
  {:else if status === "failed" || !sheet}
    <p role="alert">{m.vault_open_failed()}</p>
  {:else}
    <div class="grid">
      <table>
        <thead>
          <tr>
            <th></th>
            {#each { length: width } as _, column (column)}<th>{columnName(column)}</th>{/each}
          </tr>
        </thead>
        <tbody>
          {#each sheet.rows.slice(0, shown) as row, index (index)}
            <tr>
              <th>{index + 1}</th>
              {#each { length: width } as _, column (column)}<td>{row[column] ?? ""}</td>{/each}
            </tr>
          {/each}
        </tbody>
      </table>
      {#if sheet.rows.length > shown}
        <button type="button" class="more" onclick={() => (shown += PAGE)}
          >{m.vault_show_more_rows()}</button
        >
      {/if}
      {#if sheet.truncated}<p>{m.vault_sheet_truncated()}</p>{/if}
    </div>
    {#if sheets.length > 1}
      <div class="tabs" role="tablist">
        {#each sheets as item, index (index)}
          <button
            type="button"
            role="tab"
            aria-selected={index === current}
            class:on={index === current}
            onclick={() => {
              current = index;
              shown = PAGE;
            }}>{item.name}</button
          >
        {/each}
      </div>
    {/if}
  {/if}
</div>

<style>
  .sheet {
    display: flex;
    flex-direction: column;
    block-size: 100%;
  }

  .grid {
    flex: 1;
    min-block-size: 0;
    overflow: auto;
  }

  table {
    border-collapse: collapse;
    font-size: 12px;
    font-variant-numeric: tabular-nums;
  }

  th,
  td {
    max-inline-size: 320px;
    padding: 3px 8px;
    overflow: hidden;
    border: 1px solid var(--color-border);
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  th {
    position: sticky;
    background: var(--color-surface);
    color: var(--color-muted);
    font-weight: 500;
  }

  thead th {
    inset-block-start: 0;
    z-index: 1;
  }

  tbody th {
    inset-inline-start: 0;
    text-align: end;
  }

  .tabs {
    display: flex;
    gap: 2px;
    padding: 4px 8px;
    overflow-x: auto;
    border-block-start: 1px solid var(--color-border);
  }

  button {
    block-size: 26px;
    padding-inline: 10px;
    border: 0;
    border-radius: var(--radius-control-compact);
    background: transparent;
    color: var(--color-label-secondary);
    font: inherit;
    font-size: 12px;
  }

  button.on {
    background: var(--color-fill-active);
    color: var(--color-text);
  }

  .more {
    margin: 8px;
  }

  p {
    padding: 16px;
    color: var(--color-muted);
  }
</style>
