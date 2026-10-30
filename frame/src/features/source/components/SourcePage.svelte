<script lang="ts">
  import { ArrowLeft02Icon } from "@hugeicons/core-free-icons";
  import * as m from "$shared/i18n/messages";
  import { commands, type PageSourceView } from "$shared/ipc/bindings";
  import { surface as browser } from "$domain/surface";
  import Button from "$shared/ui/Button";
  import IconButton from "$shared/ui/IconButton";
  import { sourceLines } from "../lib/source-lines";

  let { tab }: { tab: string } = $props();

  /** `undefined` while asking; `null` is a document no longer held. */
  let source = $state<PageSourceView | null | undefined>(undefined);
  let copied = $state(false);
  let reset: ReturnType<typeof setTimeout> | undefined;
  let lifetime = 0;

  $effect(() => {
    const asked = tab;
    let active = true;
    source = undefined;
    copied = false;
    void commands
      .pageSource(asked)
      .catch(() => null)
      .then((found) => {
        if (active) source = found;
      });
    return () => {
      active = false;
      lifetime += 1;
      clearTimeout(reset);
    };
  });

  let lines = $derived(source ? sourceLines(source.text) : null);
  let digits = $derived(String(lines?.total ?? 1).length);
  const number = new Intl.NumberFormat();

  async function copy() {
    if (!source) return;
    const asked = lifetime;
    try {
      await navigator.clipboard.writeText(source.text);
    } catch {
      return;
    }
    if (asked !== lifetime) return;
    copied = true;
    clearTimeout(reset);
    reset = setTimeout(() => (copied = false), 1600);
  }
</script>

<section class="library-shell">
  <header class="internal-toolbar">
    <IconButton
      icon={ArrowLeft02Icon}
      label={m.settings_back()}
      onclick={() => void browser.close()}
    /><span class="toolbar-current">{m.source_title()}</span>
    {#if source}<span class="address" title={source.url}>{source.url}</span>{/if}
    <span class="spacer"></span>
    {#if lines}<span class="count">{m.source_lines({ count: number.format(lines.total) })}</span
      >{/if}
    {#if source}<Button size="compact" onclick={() => void copy()}
        >{copied ? m.source_copied() : m.source_copy()}</Button
      >{/if}
  </header>

  {#if source === null}
    <p class="note" role="alert">{m.source_gone()}</p>
  {:else if source && lines}
    {#if source.truncated || lines.cut}<p class="note" role="status">
        {m.source_truncated()}
      </p>{/if}
    <!-- Plain text only: each line is a text node, never markup. Focusable so the
         keyboard can scroll it. -->
    <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
    <div
      class="viewer"
      role="region"
      aria-label={m.source_title()}
      tabindex="0"
      style:--digits={digits}
    >
      {#each lines.chunks as chunk (chunk.start)}
        <div class="chunk">
          {#each chunk.lines as line, at (at)}<div class="line" data-n={chunk.start + at + 1}>
              {line}
            </div>{/each}
        </div>
      {/each}
    </div>
  {/if}
</section>

<style>
  .address {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--color-faint);
    user-select: text;
  }

  .spacer {
    flex: 1;
  }

  .count {
    flex: none;
    color: var(--color-faint);
    font-variant-numeric: tabular-nums;
  }

  .note {
    margin: 0;
    padding: 6px 16px 10px;
    color: var(--color-muted);
    font-size: var(--text-body);
  }

  .viewer {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: 4px 0 24px;
    font-family: var(--font-mono);
    font-size: 12px;
    line-height: 1.6;
    color: var(--color-text);
    tab-size: 4;
    user-select: text;
  }

  .viewer:focus-visible {
    outline: 2px solid var(--color-accent);
    outline-offset: -2px;
  }

  /* Blocks that are off screen are skipped by the browser, so a long source
     costs what is in view. */
  .chunk {
    content-visibility: auto;
    contain-intrinsic-size: auto 3200px;
  }

  .line {
    display: grid;
    grid-template-columns: calc(var(--digits) * 1ch + 24px) minmax(0, 1fr);
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    padding-inline-end: 16px;
  }

  .line::before {
    content: attr(data-n);
    padding-inline-end: 12px;
    text-align: end;
    color: var(--color-faint);
    user-select: none;
    font-variant-numeric: tabular-nums;
  }
</style>
