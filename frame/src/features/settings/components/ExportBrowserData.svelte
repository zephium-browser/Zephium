<script lang="ts">
  import * as m from "$shared/i18n/messages";
  import { exportBrowserData, type ExportKind, type ExportOutcome } from "$domain/browser-export";
  import Button from "$shared/ui/Button";
  import SettingsRow from "$shared/ui/SettingsRow";

  const rows: ReadonlyArray<{ kind: ExportKind; title: () => string; help: () => string }> = [
    { kind: "bookmarks", title: m.export_bookmarks_title, help: m.export_bookmarks_help },
    { kind: "history", title: m.export_history_title, help: m.export_history_help },
  ];

  let running = $state<ExportKind | null>(null);
  let outcome = $state<ExportOutcome | null>(null);

  async function start(kind: ExportKind) {
    if (running) return;
    running = kind;
    outcome = null;
    outcome = await exportBrowserData(kind);
    running = null;
  }
</script>

<div data-setting="general.export">
  {#each rows as row (row.kind)}
    <SettingsRow title={row.title()} description={row.help()}>
      <Button
        pending={running === row.kind}
        disabled={running !== null}
        onclick={() => void start(row.kind)}>{m.general_export_button()}</Button
      >
    </SettingsRow>
  {/each}
  {#if outcome === "saved"}<p class="result" role="status">{m.export_saved()}</p>
  {:else if outcome === "private"}<p class="result" role="status">{m.export_private()}</p>
  {:else if outcome === "failed"}<p class="result error" role="alert">{m.export_failed()}</p>{/if}
</div>

<style>
  .result {
    margin: 12px 18px;
    color: var(--color-muted);
    font-size: var(--text-body);
  }

  .error {
    color: var(--color-danger);
  }
</style>
