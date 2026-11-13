<script lang="ts">
  import { isComposing } from "$shared/lib/keyboard";
  import SubjectPicture from "../cards/SubjectPicture.svelte";
  import type { EvidenceReference } from "$shared/ui/data/Artifact";
  import HostGlyph from "../cards/HostGlyph.svelte";
  import Icon from "$shared/ui/Icon";
  import { Cancel01Icon, Tick02Icon } from "../../lib/icons";
  import { cellText, compareCsv, type CompareCell, type CompareModel } from "../../lib/compare";
  import * as m from "$shared/i18n/messages";
  let {
    model,
    correctable = false,
    onevidence,
    oncorrect,
  }: {
    model: CompareModel;
    /** Whether this result can still be corrected by the person reading it. */
    correctable?: boolean;
    /** A chip opens its source: the pane for a page, the lift for a file. */
    onevidence?: (reference: EvidenceReference) => void;
    /** Enter commits the cell the person retyped; Escape leaves it as it was. */
    oncorrect?: (cell: CompareCell, text: string) => void;
  } = $props();
  let editing = $state<string | null>(null);
  let draft = $state("");
  const at = (cell: CompareCell) => `${cell.subject}:${cell.criterion}`;
  function start(cell: CompareCell) {
    editing = at(cell);
    draft = cellText(cell.value, m.work_yes(), m.work_no());
  }
  /** The price row is taken into the headers, but it is still a row of the table. */
  const rowCount = $derived(model.rows.length + (model.priceLabel ? 1 : 0));
  let copied = $state<"idle" | "copied" | "failed">("idle");
  function copy() {
    void navigator.clipboard.writeText(compareCsv(model, m.work_yes(), m.work_no())).then(
      () => (copied = "copied"),
      () => (copied = "failed"),
    );
  }
  function commit(cell: CompareCell) {
    const text = draft;
    editing = null;
    if (text.trim() !== cellText(cell.value, m.work_yes(), m.work_no()).trim())
      oncorrect?.(cell, text);
  }
</script>

<div class="compare" tabindex="-1">
  <table style:--subjects={model.columns.length}>
    <thead>
      <tr>
        <th scope="col" class="corner"><span class="sr-only">{m.work_matrix_criterion()}</span></th>
        {#each model.columns as column (column.key)}
          <th scope="col" class="subject">
            <span class="who">
              <span class="picture">
                <SubjectPicture
                  picture={column.picture}
                  name={column.name}
                  homepage={column.homepage ?? ""}
                  markSize={20}
                />
              </span>
              <span class="name"
                >{#if column.picture && column.homepage}<span class="favicon"
                    ><HostGlyph url={column.homepage} size={12} initial={false} /></span
                  >{/if}{column.name}</span
              >
            </span>
            {#if column.price}<span class="price"
                >{#if column.best}<span
                    class="best"
                    role="img"
                    title={m.work_compare_lowest()}
                    aria-label={m.work_compare_lowest()}
                  ></span>{/if}<span>{column.price}</span></span
              >{/if}
            {#if column.descriptor}<span class="descriptor">{column.descriptor}</span>{/if}
          </th>
        {/each}
      </tr>
    </thead>
    <tbody>
      {#each model.rows as row (row.key)}
        <tr>
          <th scope="row" class="criterion">
            <span class="label">{row.label}</span>
            {#if row.meta}<span class="meta">{row.meta}</span>{/if}
          </th>
          {#each row.cells as cell (cell.subject)}
            <td
              class:numeric={row.numeric}
              class:check={row.check}
              class:unknown={cell.value.kind === "unknown"}
            >
              <span class="cell">
                {#if editing === at(cell)}
                  <!-- svelte-ignore a11y_autofocus -->
                  <input
                    class="editor"
                    autofocus
                    bind:value={draft}
                    aria-label={`${row.label}, ${model.columns[cell.subject]?.name ?? ""}`}
                    maxlength="512"
                    onkeydown={(event) => {
                      if (isComposing(event)) return;
                      if (event.key === "Enter") {
                        event.preventDefault();
                        commit(cell);
                      } else if (event.key === "Escape") {
                        event.preventDefault();
                        editing = null;
                      }
                    }}
                    onblur={() => (editing = null)}
                  />
                {:else if cell.value.kind === "unknown"}<span class="dash" aria-hidden="true"
                    >—</span
                  ><span class="sr-only">{m.work_cell_unknown()}</span>
                {:else if cell.value.kind === "mark"}<span
                    class="glyph"
                    class:yes={cell.value.yes}
                    aria-label={cell.value.yes ? m.work_yes() : m.work_no()}
                    ><Icon icon={cell.value.yes ? Tick02Icon : Cancel01Icon} size={15} /></span
                  >
                {:else}{#if cell.best}<span
                      class="best"
                      role="img"
                      title={m.work_compare_best()}
                      aria-label={m.work_compare_best()}
                    ></span>{/if}<span class="text">{cell.value.text}</span>{/if}
                {#if cell.share !== undefined && editing !== at(cell)}<span
                    class="bar"
                    aria-hidden="true"
                    ><i style:inline-size={`${Math.round(cell.share * 100)}%`}></i></span
                  >{/if}
                {#if cell.note}<span class="note">{cell.note}</span>{/if}
                <span class="aside">
                  {#each cell.evidence as reference (reference.key)}
                    <button
                      type="button"
                      class="chip"
                      title={reference.label}
                      onclick={() => onevidence?.(reference)}
                    >
                      <HostGlyph
                        host={reference.origin ?? ""}
                        url={reference.url}
                        file={!!reference.file}
                        size={14}
                      />
                      <span>{reference.origin || reference.label}</span>
                    </button>
                  {/each}
                  {#if correctable && editing !== at(cell)}<button
                      type="button"
                      class="correct"
                      onclick={() => start(cell)}>{m.work_env_correct()}</button
                    >{/if}
                </span>
              </span>
            </td>
          {/each}
        </tr>
      {/each}
    </tbody>
  </table>
  {#if model.notes.length}
    <ul class="notes">
      {#each model.notes as note, index (index)}<li>{note}</li>{/each}
    </ul>
  {/if}
</div>
<footer class="compare-footer">
  <span class="count"
    >{rowCount === 1 ? m.work_table_count_one() : m.work_table_count({ count: rowCount })}</span
  >
  {#if copied === "failed"}<span class="failed" role="alert">{m.work_code_copy_failed()}</span>{/if}
  <button type="button" class="copy" onclick={copy}
    >{copied === "copied" ? m.work_code_copied() : m.work_table_copy_csv()}</button
  >
</footer>

<style>
  .compare {
    min-inline-size: 0;
    overflow: auto;
  }

  /* Even columns, never narrower than a name and a price; the lift scrolls sideways. */
  table {
    table-layout: fixed;
    border-collapse: separate;
    border-spacing: 0;
    inline-size: max(100%, calc(150px + var(--subjects) * 180px));
    font-size: var(--text-body);
    line-height: 19px;
  }

  th,
  td {
    vertical-align: top;
    padding: 10px 12px;
    border-block-end: 1px solid var(--color-border);
    text-align: start;
  }

  thead th {
    position: sticky;
    inset-block-start: 0;
    z-index: 2;
    background: var(--color-surface);
    border-block-end: 1px solid var(--color-border-strong);
  }

  .corner {
    inset-inline-start: 0;
    z-index: 3;
    inline-size: 150px;
    min-inline-size: 150px;
  }

  /* The row under the pointer reads across; the sticky column keeps its surface under the tint. */
  tbody tr:hover > * {
    background-image: linear-gradient(var(--color-fill), var(--color-fill));
  }

  .best {
    display: inline-block;
    inline-size: 6px;
    block-size: 6px;
    margin-inline-end: 6px;
    border-radius: 50%;
    background: var(--color-accent);
    vertical-align: 0.1em;
  }

  .subject {
    min-inline-size: 160px;
  }

  /* A 40 px tile, then the name beside it: the column reads as the card does. */
  .who {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-block-end: 6px;
  }

  .picture {
    display: block;
    flex: none;
    inline-size: 40px;
    block-size: 40px;
    border-radius: var(--radius-inset);
    background: var(--color-fill);
    box-shadow: inset 0 0 0 1px var(--color-border);
    overflow: hidden;
  }

  .name {
    display: -webkit-box;
    -webkit-box-orient: vertical;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    min-inline-size: 0;
    overflow: hidden;
    font-size: var(--text-body);
    font-weight: 600;
    line-height: 17px;
  }

  .favicon {
    display: inline-flex;
    margin-inline-end: 5px;
    vertical-align: -1px;
  }

  .price {
    display: block;
    margin-block-start: 2px;
    font-size: var(--text-body);
    font-weight: 600;
    font-variant-numeric: tabular-nums;
  }

  .criterion {
    position: sticky;
    inset-inline-start: 0;
    z-index: 1;
    background: var(--color-surface);
  }

  .label {
    display: block;
    color: var(--color-muted);
    font-weight: 500;
  }

  .descriptor,
  .meta,
  .note {
    display: block;
    color: var(--color-faint);
    font-size: var(--text-caption);
    font-weight: 400;
  }

  .cell {
    display: block;
    min-block-size: 20px;
  }

  .text {
    display: -webkit-box;
    -webkit-box-orient: vertical;
    -webkit-line-clamp: 3;
    line-clamp: 3;
    overflow: hidden;
  }

  td.numeric .cell {
    text-align: end;
    font-variant-numeric: tabular-nums;
  }

  .dash {
    color: var(--color-faint);
  }

  .bar {
    display: block;
    block-size: 4px;
    margin-block-start: 6px;
    border-radius: 2px;
    background: var(--color-track);
    overflow: hidden;
  }

  .bar i {
    display: block;
    block-size: 100%;
    margin-inline-start: auto;
    border-radius: inherit;
    background: var(--color-label-secondary);
  }

  .glyph {
    display: inline-grid;
    place-items: center;
    color: var(--color-faint);
  }

  .glyph.yes {
    color: var(--color-success);
  }

  .aside {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: inherit;
    gap: 4px;
    margin-block-start: 5px;
    opacity: 0;
    transition: opacity var(--motion-fast) var(--ease-smooth);
  }

  td:hover .aside,
  td:focus-within .aside {
    opacity: 1;
  }

  td.numeric .text {
    display: inline;
  }

  td.check .cell {
    text-align: center;
  }

  td.check .aside {
    justify-content: center;
  }

  .chip {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    max-inline-size: 120px;
    padding: 1px 7px 1px 2px;
    border: 0;
    border-radius: var(--radius-capsule);
    background: var(--color-fill);
    color: var(--color-muted);
    font: inherit;
    font-size: var(--text-caption);
    cursor: default;
  }

  .chip span {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .chip:hover {
    background: var(--color-fill-hover);
    color: var(--color-text);
  }

  .chip:focus-visible {
    outline: 2px solid var(--color-ring);
    outline-offset: 1px;
  }

  .editor {
    inline-size: 100%;
    box-sizing: border-box;
    padding: 2px 6px;
    border: 0;
    border-radius: var(--radius-inset);
    background: var(--color-field);
    color: var(--color-text);
    font: inherit;
    outline: 2px solid var(--color-ring);
  }

  .correct {
    padding: 1px 8px;
    border: 0;
    border-radius: var(--radius-capsule);
    background: transparent;
    box-shadow: inset 0 0 0 1px var(--color-border-strong);
    color: var(--color-muted);
    font: inherit;
    font-size: var(--text-caption);
    cursor: default;
  }

  .correct:hover {
    background: var(--color-fill-hover);
    color: var(--color-text);
  }

  .correct:focus-visible {
    outline: 2px solid var(--color-ring);
    outline-offset: 1px;
  }

  .notes {
    margin: 12px 0 0;
    padding-inline-start: 18px;
    color: var(--color-muted);
    font-size: var(--text-caption);
  }

  .compare-footer {
    display: flex;
    align-items: center;
    gap: 12px;
    margin-block-start: 8px;
    color: var(--color-muted);
    font-size: var(--text-caption);
  }

  .count {
    flex: 1;
  }

  .failed {
    color: var(--color-danger);
  }

  .copy {
    padding: 2px 10px;
    border: 0;
    border-radius: var(--radius-capsule);
    background: var(--color-control);
    color: var(--color-text);
    font: inherit;
    cursor: default;
    transition: background-color var(--motion-fast) var(--ease-out);
  }

  .copy:hover {
    background: var(--color-control-hover);
  }

  .copy:focus-visible {
    outline: 2px solid var(--color-ring);
    outline-offset: 2px;
  }

  .sr-only {
    position: absolute;
    inline-size: 1px;
    block-size: 1px;
    overflow: hidden;
    clip-path: inset(50%);
  }
</style>
