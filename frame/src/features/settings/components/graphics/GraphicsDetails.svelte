<script lang="ts">
  import * as m from "$shared/i18n/messages";
  import * as notices from "$session/notice.svelte";
  import SettingsRow from "$shared/ui/SettingsRow";
  import Button from "$shared/ui/Button";
  import Icon from "$shared/ui/Icon";
  import { Alert02Icon } from "@hugeicons/core-free-icons";
  import { graphicsDetails, graphicsWarnings } from "../../lib/graphics";
  import type { Graphics } from "../../lib/graphics-state.svelte";

  let { graphics }: { graphics: Graphics } = $props();
  const uid = $props.id();

  // Shown before the report arrives, so the card keeps its height.
  const LABELS = [
    m.graphics_driver,
    m.graphics_gpu,
    m.graphics_opengl,
    m.graphics_driver_version,
    m.graphics_session,
    m.graphics_engine,
    m.graphics_selected,
  ];

  let report = $derived(graphics.report());
  let rows = $derived(report ? graphicsDetails(report) : null);
  let warnings = $derived(report ? graphicsWarnings(report) : []);
  let reading = $state<"copy" | "review" | null>(null);
  let outcome = $state<"failed" | "blocked" | null>(null);
  let reviewing = $state(false);
  let text = $state<string | null>(null);

  const fresh = () => graphics.read().catch(() => null);

  /** The clipboard is written inside the click, with the text still to come:
   *  WebKit refuses a write that waits for the native read first. */
  async function copy() {
    reading = "copy";
    outcome = null;
    const read = fresh();
    let copied = false;
    try {
      const plain = read.then((next) => {
        if (next === null) throw new Error("unavailable");
        return new Blob([next.text], { type: "text/plain" });
      });
      await navigator.clipboard.write([new ClipboardItem({ "text/plain": plain })]);
      copied = true;
    } catch {
      // Tried again below, as text, once the report is in hand.
    }
    const next = await read;
    reading = null;
    if (next === null) {
      outcome = "failed";
      return;
    }
    graphics.adopt(next);
    text = next.text;
    if (!copied) {
      copied = await navigator.clipboard.writeText(next.text).then(
        () => true,
        () => false,
      );
    }
    if (copied) notices.show(m.graphics_report_copied());
    else {
      outcome = "blocked";
      reviewing = true;
    }
  }

  async function review() {
    if (reviewing) {
      reviewing = false;
      return;
    }
    reading = "review";
    outcome = null;
    const next = await fresh();
    reading = null;
    if (next === null) {
      outcome = "failed";
      return;
    }
    graphics.adopt(next);
    text = next.text;
    reviewing = true;
  }
</script>

<dl class="facts" aria-busy={!rows || undefined}>
  {#if rows}
    {#each rows as row (row.id)}
      <div class="fact" data-fact={row.id}>
        <dt>{row.label}</dt>
        <dd>
          <span class="value">{row.value}</span>
          {#if row.note}<span class="note" lang="en">{row.note}</span>{/if}
        </dd>
      </div>
    {/each}
  {:else}
    {#each LABELS as label, index (index)}
      <div class="fact">
        <dt>{label()}</dt>
        <dd>
          {#if graphics.failed()}<span class="value muted">{m.graphics_unknown()}</span>{:else}<span
              class="placeholder"
              aria-hidden="true"
            ></span>{/if}
        </dd>
      </div>
    {/each}
  {/if}
</dl>
{#if warnings.length}
  <div class="warnings">
    <h3 id="{uid}-warnings">{m.graphics_warnings()}</h3>
    <ul aria-labelledby="{uid}-warnings">
      {#each warnings as warning, index (index)}
        <li>
          <span class="glyph" aria-hidden="true"><Icon icon={Alert02Icon} size={14} /></span><span
            lang={warning.native ? "en" : undefined}>{warning.text}</span
          >
        </li>
      {/each}
    </ul>
  </div>
{/if}
<SettingsRow
  settingId="graphics.report"
  title={m.graphics_report()}
  description={outcome === "failed"
    ? m.graphics_report_failed()
    : outcome === "blocked"
      ? m.about_diagnostics_blocked()
      : m.graphics_report_help()}
>
  <div class="actions">
    <Button
      size="compact"
      variant="ghost"
      pending={reading === "review"}
      disabled={reading === "copy" || (!report && !reviewing)}
      aria-expanded={reviewing}
      aria-controls="{uid}-report"
      onclick={() => void review()}
      >{reviewing ? m.about_diagnostics_hide() : m.about_diagnostics_review()}</Button
    >
    <Button
      size="compact"
      pending={reading === "copy"}
      disabled={reading === "review" || !report}
      onclick={() => void copy()}>{m.graphics_report_copy()}</Button
    >
  </div>
</SettingsRow>
{#if reviewing && text !== null}
  <div class="report" id="{uid}-report">
    <textarea readonly rows="12" spellcheck="false" lang="en" aria-label={m.graphics_report()}
      >{text}</textarea
    >
  </div>
{/if}

<style>
  .facts {
    display: grid;
    grid-template-columns: minmax(120px, max-content) minmax(0, 1fr);
    margin: 0;
    padding: 6px 0;
  }

  .fact {
    display: grid;
    grid-column: 1 / -1;
    grid-template-columns: subgrid;
    column-gap: 24px;
    padding: 7px 18px;
  }

  dt {
    color: var(--color-muted);
    font-size: var(--text-label);
    line-height: 19px;
  }

  dd {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-inline-size: 0;
    margin: 0;
  }

  .value {
    color: var(--color-text);
    font-size: var(--text-label);
    font-weight: 500;
    line-height: 19px;
    overflow-wrap: anywhere;
    font-variant-numeric: tabular-nums;
    user-select: text;
  }

  .value.muted {
    color: var(--color-muted);
    font-weight: 400;
  }

  .note {
    color: var(--color-muted);
    font-family: var(--font-mono);
    font-size: var(--text-caption);
    line-height: 1.5;
    overflow-wrap: anywhere;
    user-select: text;
  }

  .placeholder {
    display: block;
    inline-size: 44%;
    block-size: 10px;
    margin-block: 4.5px;
    border-radius: 4px;
    background: var(--color-fill-strong);
  }

  .warnings {
    position: relative;
    padding: 12px 18px 14px;
  }

  .warnings::before {
    content: "";
    position: absolute;
    inset-inline: 16px 0;
    top: 0;
    height: 1px;
    background: var(--color-border);
  }

  .warnings h3 {
    margin: 0 0 6px;
    color: var(--color-text);
    font-size: var(--text-label);
    font-weight: 600;
    line-height: 19px;
  }

  .warnings ul {
    display: grid;
    gap: 4px;
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .warnings li {
    display: flex;
    align-items: flex-start;
    gap: 8px;
    color: var(--color-label-secondary);
    font-size: var(--text-label);
    line-height: 1.5;
    overflow-wrap: anywhere;
  }

  .warnings .glyph {
    display: grid;
    flex: none;
    place-items: center;
    block-size: 18px;
    color: var(--color-warning);
  }

  /* The report row draws its own divider only against another row. */
  .facts + :global(.ui-settings-row)::before,
  .warnings + :global(.ui-settings-row)::before {
    content: "";
    position: absolute;
    inset-inline: 16px 0;
    top: 0;
    height: 1px;
    background: var(--color-border);
  }

  .actions {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: flex-end;
    gap: 6px;
  }

  .report {
    padding: 0 18px 14px;
  }

  .report textarea {
    box-sizing: border-box;
    inline-size: 100%;
    margin: 0;
    padding: 12px 14px;
    resize: vertical;
    border: 0;
    border-radius: var(--radius-field);
    background: var(--color-field);
    color: var(--color-text);
    font-family: var(--font-mono);
    font-size: var(--text-label);
    line-height: 1.55;
    white-space: pre;
    overflow-wrap: normal;
  }

  .report textarea:focus-visible {
    outline: 2px solid var(--color-ring);
    outline-offset: 2px;
  }

  @container (max-width: 450px) {
    .facts {
      grid-template-columns: minmax(0, 1fr);
    }

    .fact {
      row-gap: 1px;
    }

    .actions {
      justify-content: flex-start;
    }
  }

  @media (forced-colors: active) {
    .warnings::before,
    .facts + :global(.ui-settings-row)::before,
    .warnings + :global(.ui-settings-row)::before {
      background: CanvasText;
    }

    .placeholder {
      border: 1px solid CanvasText;
    }

    .warnings .glyph {
      color: CanvasText;
    }

    .report textarea {
      border: 1px solid CanvasText;
    }
  }
</style>
