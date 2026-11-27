<script lang="ts">
  import * as m from "$shared/i18n/messages";
  import { tabs } from "$domain/tabs";
  import Button from "$shared/ui/Button";
  import Icon from "$shared/ui/Icon";
  import { commands, type PageAssistBlockV1 } from "$shared/ipc/bindings";
  import type { AssistSession } from "../lib/assist-session.svelte";
  import { replyText, type Reply } from "../lib/conversation";
  import { costLine } from "../lib/format";
  import { Copy01Icon, HandIcon, InformationCircleIcon, Tick02Icon } from "../lib/icons";
  import FaultNote from "./FaultNote.svelte";
  import WorkingLine from "./WorkingLine.svelte";
  import HandoffCard from "./blocks/HandoffCard.svelte";
  import ItemsBlock from "./blocks/ItemsBlock.svelte";
  import OpenedBlock from "./blocks/OpenedBlock.svelte";
  import StepsBlock from "./blocks/StepsBlock.svelte";
  import SearchBlock from "./blocks/SearchBlock.svelte";
  import StyleBlock from "./blocks/StyleBlock.svelte";
  import TextBlock from "./blocks/TextBlock.svelte";
  import "$shared/ui/Shine/shine.css";
  import "./assist.css";

  let {
    reply,
    session,
    latest,
    onwork,
  }: {
    reply: Reply;
    session: AssistSession;
    /** The conversation's last reply: it alone shows what it cost. */
    latest: boolean;
    /** Shows a started Work in Work mode. */
    onwork: (work: string) => void;
  } = $props();

  /** A block that shows its own progress: the reply's line steps aside, so only one thing moves. */
  const busyBlock = (block: PageAssistBlockV1) =>
    (block.kind === "opened" && block.tabs.some((row) => row.state === "opening")) ||
    (block.kind === "steps" && block.steps.some((step) => step.state === "running"));
  const working = $derived(reply.state === "working" && !reply.blocks.some(busyBlock));
  const settled = $derived(
    reply.state === "done" || reply.state === "failed" || reply.state === "cancelled",
  );
  const words = $derived(replyText(reply.blocks));
  const site = $derived(session.status?.page?.host ?? null);

  let copied = $state(false);
  async function copy() {
    try {
      await navigator.clipboard.writeText(words);
      copied = true;
      setTimeout(() => (copied = false), 1600);
    } catch {
      // A denied clipboard leaves the words on screen to select.
    }
  }

  function focus(block: number, tab: string, n: number | null, pressed = false) {
    if (pressed && tab !== tabs.activeId()) tabs.activate(tab);
    session.focus(reply.id, block, n);
  }

  function storedStyle(id: string | null, site: string) {
    const held = session.styles;
    if (!held || held.site !== site) return undefined;
    return held.styles.find((style) => style.id === id) ?? null;
  }
</script>

<article class="reply" aria-label={m.assist_reply_label()} aria-busy={reply.state === "working"}>
  {#each reply.blocks as block, index (index)}
    <div class="block" class:assist-rise={block.kind !== "text"}>
      {#if block.kind === "text"}
        <TextBlock
          text={block.text}
          cites={block.cites}
          arrivals={session.arrivals[`${reply.id}:${index}`]}
          inert={reply.inert}
          oncite={(cite) => session.cite(reply.id, cite, tabs.activeId() ?? null)}
        />
      {:else if block.kind === "items"}
        {@const tab = block.tab}
        <ItemsBlock
          items={block.items}
          onfocus={(n, pressed) => focus(index, tab, n, pressed)}
          onopen={(url) => void commands.browserOpenUrl(url, true).catch(() => {})}
        />
      {:else if block.kind === "opened"}
        <OpenedBlock rows={block.tabs} />
      {:else if block.kind === "steps"}
        <StepsBlock
          steps={block.steps}
          {site}
          ondecide={(decision) => session.decide(reply.id, index, decision)}
        />
      {:else if block.kind === "search"}
        <SearchBlock
          destination={block.destination}
          query={block.query}
          state={block.state}
          inert={reply.inert || reply.state !== "waiting"}
          ondecide={(decision) => session.decide(reply.id, index, decision)}
        />
      {:else if block.kind === "paused"}
        <section class="assist-card ui-shine paused" aria-label={m.assist_paused_title()}>
          <div class="head">
            <span class="plate" aria-hidden="true"><Icon icon={HandIcon} size={15} /></span>
            <h4>{m.assist_paused_title()}</h4>
          </div>
          <p>{m.assist_paused_help()}</p>
          {#if reply.state === "paused"}<div class="assist-actions">
              <Button size="compact" onclick={() => session.resume(reply.id)}
                >{m.assist_continue()}</Button
              >
            </div>{/if}
        </section>
      {:else if block.kind === "style"}
        {@const style = block.style}
        <StyleBlock
          {style}
          stored={storedStyle(style.id, style.site)}
          ondecide={(decision) => session.decide(reply.id, index, decision)}
          ontoggle={(enabled) => style.id && void session.toggleStyle(style.id, enabled)}
          onremove={() => style.id && void session.removeStyle(style.id)}
        />
      {:else if block.kind === "handoff"}
        <HandoffCard
          handoff={block.handoff}
          oncontinue={() => void session.continueWork(reply.id, index)}
          ondecline={() => session.decide(reply.id, index, "decline")}
          onopen={onwork}
        />
      {/if}
    </div>
  {/each}
  {#if working}<WorkingLine phase={reply.phase} />{/if}
  {#if reply.state === "failed" && reply.fault && !(reply.fault.reason === "step_failed" && reply.blocks.some((block) => block.kind === "steps" && block.steps.some((step) => step.state === "failed")))}
    <FaultNote
      fault={reply.fault}
      onretry={latest ? () => void session.retry(reply.id) : undefined}
    />
  {:else if reply.state === "cancelled"}
    <p class="assist-quiet">{m.assist_stopped()}</p>
  {/if}
  {#if settled && words}
    <footer class="foot">
      <button type="button" class="tool" onclick={copy}>
        <Icon icon={copied ? Tick02Icon : Copy01Icon} size={13} strokeWidth={1.8} />{copied
          ? m.assist_copied()
          : m.assist_copy()}
      </button>
      {#if latest && reply.cost}
        <span class="dot" aria-hidden="true">·</span>
        <span class="tool cost" title={costLine(reply.cost)} aria-label={costLine(reply.cost)}>
          <Icon icon={InformationCircleIcon} size={13} strokeWidth={1.8} />{m.assist_cost()}
        </span>
      {/if}
    </footer>
  {/if}
</article>

<style>
  .reply {
    position: relative;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .block {
    min-inline-size: 0;
  }

  .paused .head {
    display: flex;
    align-items: center;
    gap: 9px;
  }

  .plate {
    display: grid;
    flex: none;
    place-items: center;
    inline-size: 26px;
    block-size: 26px;
    border-radius: var(--radius-inset);
    background: var(--color-field);
    color: var(--color-muted);
  }

  /* Copy and what it cost: there when the pointer is on the reply, quiet otherwise. */
  .foot {
    display: flex;
    align-items: center;
    gap: 2px;
    block-size: 24px;
    margin-block-start: -4px;
    margin-inline-start: -6px;
    opacity: 0;
    transition: opacity var(--motion-fast) var(--ease-out);
  }

  .foot:focus-within {
    opacity: 1;
  }

  .reply:hover .foot {
    opacity: 1;
  }

  .tool {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    block-size: 24px;
    padding: 0 7px;
    border: 0;
    border-radius: var(--radius-capsule);
    background: transparent;
    color: var(--color-muted);
    font: inherit;
    font-size: 12px;
    cursor: default;
    transition:
      background-color var(--motion-instant) var(--ease-smooth),
      color var(--motion-instant) var(--ease-smooth);
  }

  button.tool:hover {
    background: var(--color-fill-hover);
    color: var(--color-text);
  }

  .tool:focus-visible {
    outline: 2px solid var(--color-ring);
  }

  .cost {
    color: var(--color-faint);
  }

  .dot {
    color: var(--color-faint);
    font-size: 12px;
  }

  @media (hover: none) {
    .foot {
      opacity: 1;
    }
  }
</style>
