<script lang="ts">
  import Button from "$shared/ui/Button";
  import Icon from "$shared/ui/Icon";
  import * as m from "$shared/i18n/messages";
  import HostGlyph from "../cards/HostGlyph.svelte";
  import { thumbnail } from "../../lib/frame-thumbs";
  import AskReceipt from "./AskReceipt.svelte";
  import AskShell from "./AskShell.svelte";
  import { siteName, type ConfirmAsk } from "./asks";
  import type { ConfirmDecision } from "./actions";
  import { Alert02Icon } from "./icons";

  /**
   * A step that commits something as the person: every word on it is Rust's
   * reading of the page as it stands, never the agent's.
   */
  let {
    ask,
    placement = "canvas",
    busy = false,
    ondecide,
    onopenpage,
  }: {
    ask: ConfirmAsk;
    placement?: "canvas" | "island";
    busy?: boolean;
    ondecide: (decision: ConfirmDecision) => void;
    onopenpage?: () => void;
  } = $props();

  const what = $derived(ask.headline.replace(/\s+as you\?$|\?$/u, ""));
  /** The button already says what it presses; the line is for when it can't. */
  const says = $derived(ask.action.trim() !== `press ${ask.verb}`);
  /** A total the page states is what the person is agreeing to pay. */
  const total = $derived(
    ask.facts.findLastIndex((fact) => /\b(total|razem|suma)\b/iu.test(fact.label)),
  );
  const LONG = 280;
  let whole = $state(false);
  const long = $derived((ask.text?.length ?? 0) > LONG || (ask.text?.split("\n").length ?? 0) > 6);

  const WORKING = {
    communication: m.work_ask_sending,
    purchase: m.work_ask_confirming,
    destructive: m.work_ask_deleting,
    save: m.work_ask_saving,
    edit: m.work_ask_saving,
    type: m.work_ask_typing,
  } as const;
  const DONE = {
    communication: m.work_ask_sent,
    purchase: m.work_ask_done,
    destructive: m.work_ask_deleted,
    save: m.work_ask_saved,
    edit: m.work_ask_saved,
    type: m.work_ask_typed,
  } as const;
  const DECLINED = {
    communication: m.work_ask_not_sent,
    purchase: m.work_ask_not_done,
    destructive: m.work_ask_kept,
    save: m.work_ask_not_saved,
    edit: m.work_ask_not_saved,
    type: m.work_ask_not_typed,
  } as const;
</script>

{#if ask.state === "open"}
  <AskShell
    {placement}
    {busy}
    label={ask.headline}
    note={says ? m.work_ask_will({ action: ask.action }) : null}
    where={ask.category === "type"
      ? siteName(ask.site)
      : m.work_ask_as_you({ site: siteName(ask.site) })}
    title={ask.headline}
  >
    {#snippet mark()}<HostGlyph host={ask.site} size={18} initial={false} />{/snippet}
    {#if ask.frame || ask.text || ask.facts.length || ask.provenance.length}
      {#if ask.frame}
        <button
          type="button"
          class="page"
          title={m.work_ask_open_page()}
          aria-label={m.work_ask_open_page()}
          disabled={!onopenpage}
          onclick={() => onopenpage?.()}
          ><canvas use:thumbnail={{ url: ask.frame, width: 360 }}></canvas></button
        >
      {/if}
      {#if ask.text}
        <blockquote class:clamped={long && !whole}>{ask.text}</blockquote>
        {#if long}<button type="button" class="more" onclick={() => (whole = !whole)}
            >{whole ? m.work_ask_show_less() : m.work_ask_show_all()}</button
          >{/if}
      {/if}
      {#if ask.facts.length}
        <dl>
          {#each ask.facts as fact, index (index)}{#if index === total && index > 0}<span
                class="rule"
              ></span>{/if}
            <dt class:total={index === total}>{fact.label}</dt>
            <dd class:total={index === total}>{fact.value}</dd>{/each}
        </dl>
      {/if}
      {#if ask.provenance.length}
        <p class="quotes">
          <Icon icon={Alert02Icon} size={13} />
          <span>{m.work_ask_includes_text({ sites: ask.provenance.join(", ") })}</span>
        </p>
      {/if}
    {/if}
    {#snippet actions()}
      <Button variant="ghost" disabled={busy} onclick={() => ondecide("decline")}
        >{m.work_ask_not_now()}</Button
      >
      {#if ask.runOption}<Button disabled={busy} onclick={() => ondecide("allow_run")}
          >{m.work_ask_allow_run()}</Button
        >{/if}
      <Button variant="primary" pending={busy} onclick={() => ondecide("approve")}
        >{ask.verb}</Button
      >
    {/snippet}
  </AskShell>
{:else if ask.state === "working"}
  <AskReceipt tone="working" status={WORKING[ask.category]()} text={what} />
{:else if ask.state === "done"}
  <AskReceipt
    tone="done"
    status={DONE[ask.category]()}
    text={what}
    detail={ask.allowedForRun
      ? ask.category === "type"
        ? m.work_ask_allowed_typing_run({ site: ask.site })
        : m.work_ask_allowed_run({ site: ask.site })
      : null}
  />
{:else if ask.state === "declined"}
  <AskReceipt tone="declined" status={DECLINED[ask.category]()} text={what} />
{:else}
  <AskReceipt
    tone="failed"
    status={ask.state === "failed"
      ? DECLINED[ask.category]()
      : ask.state === "gone"
        ? m.work_ask_closed()
        : m.work_ask_unsure()}
    text={what}
    detail={ask.note}
  />
{/if}

<style>
  /* The page as it stands, small: the person sees where it happens. */
  .page {
    display: block;
    box-sizing: border-box;
    inline-size: 100%;
    block-size: 112px;
    padding: 0;
    overflow: hidden;
    border: 0;
    border-radius: var(--radius-inset);
    background: var(--color-surface);
    outline: 0.5px solid var(--color-border);
    outline-offset: -0.5px;
    cursor: default;
  }

  .page canvas {
    display: block;
    inline-size: 100%;
    block-size: 100%;
    object-fit: cover;
    object-position: top;
    pointer-events: none;
  }

  .page:focus-visible {
    outline: 2px solid var(--color-ring);
    outline-offset: 2px;
  }

  /* The exact words, as the page holds them. */
  blockquote {
    margin: 0;
    padding: 1px 0 1px 10px;
    border-inline-start: 2px solid var(--color-border-strong);
    color: var(--color-text);
    font-size: var(--text-body);
    line-height: 18px;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }

  blockquote.clamped {
    display: -webkit-box;
    overflow: hidden;
    -webkit-box-orient: vertical;
    -webkit-line-clamp: 6;
    line-clamp: 6;
  }

  .more {
    align-self: flex-start;
    margin-block-start: -4px;
    padding: 0;
    border: 0;
    background: transparent;
    color: var(--color-muted);
    font: inherit;
    font-size: var(--text-label);
    cursor: default;
  }

  .more:hover {
    color: var(--color-text);
  }

  .more:focus-visible {
    outline: 2px solid var(--color-ring);
    outline-offset: 2px;
  }

  dl {
    display: grid;
    grid-template-columns: minmax(0, auto) minmax(0, 1fr);
    gap: 4px 14px;
    margin: 0;
    font-size: var(--text-label);
    line-height: 16px;
  }

  dt {
    color: var(--color-muted);
    overflow-wrap: anywhere;
  }

  dd {
    margin: 0;
    color: var(--color-text);
    font-variant-numeric: tabular-nums;
    text-align: end;
    overflow-wrap: anywhere;
  }

  dt.total,
  dd.total {
    color: var(--color-text);
    font-weight: 600;
  }

  .rule {
    grid-column: 1 / -1;
    block-size: 1px;
    margin-block: 2px;
    background: var(--color-border);
  }

  .quotes {
    display: flex;
    align-items: flex-start;
    gap: 6px;
    margin: 0;
    color: var(--color-warning);
    font-size: var(--text-label);
    line-height: 16px;
  }

  .quotes :global(svg) {
    flex: none;
    margin-block-start: 1.5px;
  }
</style>
