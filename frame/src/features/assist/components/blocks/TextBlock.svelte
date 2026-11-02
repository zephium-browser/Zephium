<script lang="ts">
  import * as m from "$shared/i18n/messages";
  import { commands, type PageAssistCiteV1 } from "$shared/ipc/bindings";
  import { parse, type Arrival, type Block, type Inline } from "../../lib/markdown";

  let {
    text,
    cites,
    arrivals = [],
    inert = false,
    oncite,
  }: {
    text: string;
    cites: readonly PageAssistCiteV1[];
    /** The latest deltas while the text streams. */
    arrivals?: readonly Arrival[];
    /** The pages it read changed since: its cites no longer jump. */
    inert?: boolean;
    /** False when the words could not be found on the page. */
    oncite: (cite: PageAssistCiteV1) => Promise<boolean>;
  } = $props();

  const tree = $derived(parse(text, arrivals));
  const byNumber = $derived(new Map(cites.map((cite) => [cite.n, cite])));

  let root = $state<HTMLElement>();
  /** The cite under the pointer or the focus, and where its card stands. */
  let peek = $state<{
    cite: PageAssistCiteV1;
    x: number;
    y: number;
    width: number;
    below: boolean;
  } | null>(null);
  let missed = $state<number | null>(null);

  function show(cite: PageAssistCiteV1, target: HTMLElement) {
    const host = root?.closest<HTMLElement>("[data-assist-thread]") ?? root;
    if (!host) return;
    const bounds = host.getBoundingClientRect();
    const chip = target.getBoundingClientRect();
    const width = Math.min(248, bounds.width - 24);
    const x = Math.max(
      bounds.left + 12,
      Math.min(chip.left + chip.width / 2 - width / 2, bounds.right - 12 - width),
    );
    const below = chip.top - bounds.top < 96;
    peek = { cite, x, y: below ? chip.bottom + 6 : chip.top - 6, width, below };
  }

  async function jump(cite: PageAssistCiteV1) {
    if (inert) return;
    const found = await oncite(cite);
    if (found) return;
    missed = cite.n;
    setTimeout(() => {
      if (missed === cite.n) missed = null;
    }, 2400);
  }

  function open(event: MouseEvent, href: string) {
    event.preventDefault();
    void commands.browserOpenUrl(href, true).catch(() => {});
  }
</script>

{#snippet inline(
  nodes: readonly Inline[],
)}{#each nodes as node, index (node.kind === "fresh" ? `f${node.key}` : index)}{#if node.kind === "text"}{node.text}{:else if node.kind === "fresh"}<span
        class="fresh">{node.text}</span
      >{:else if node.kind === "strong"}<strong>{@render inline(node.children)}</strong
      >{:else if node.kind === "em"}<em>{@render inline(node.children)}</em
      >{:else if node.kind === "del"}<del>{@render inline(node.children)}</del
      >{:else if node.kind === "code"}<code>{node.text}</code>{:else if node.kind === "link"}<a
        href={node.href}
        title={node.href}
        target="_blank"
        rel="noreferrer noopener"
        onclick={(event) => open(event, node.href)}>{@render inline(node.children)}</a
      >{:else if node.kind === "break"}<br />{:else if node.kind === "cite"}{@const cite =
        byNumber.get(node.n)}{#if cite}<span class="glue"
          ><button
            type="button"
            class="cite"
            data-missed={missed === cite.n || undefined}
            aria-disabled={inert || undefined}
            aria-label={m.assist_cite_label({ n: cite.n, excerpt: cite.excerpt })}
            onpointerenter={(event) => show(cite, event.currentTarget)}
            onpointerleave={() => (peek = null)}
            onfocus={(event) => show(cite, event.currentTarget)}
            onblur={() => (peek = null)}
            onclick={() => void jump(cite)}>{cite.n}</button
          >{node.after}</span
        >{:else}{node.after}{/if}{/if}{/each}{/snippet}

{#snippet blocks(nodes: readonly Block[])}
  {#each nodes as node, index (index)}
    {#if node.kind === "paragraph"}<p>{@render inline(node.children)}</p>
    {:else if node.kind === "heading"}<p class="heading">{@render inline(node.children)}</p>
    {:else if node.kind === "list"}
      {#if node.ordered}<ol start={node.start}>
          {#each node.items as item, at (at)}<li>{@render blocks(item)}</li>{/each}
        </ol>
      {:else}<ul>
          {#each node.items as item, at (at)}<li>{@render blocks(item)}</li>{/each}
        </ul>{/if}
    {:else if node.kind === "code"}<pre><code>{node.text}</code></pre>
    {:else if node.kind === "quote"}<blockquote>{@render blocks(node.children)}</blockquote>
    {:else}<hr />{/if}
  {/each}
{/snippet}

<div class="text" data-selectable bind:this={root}>
  {@render blocks(tree)}
</div>

{#if peek}
  {@const cite = peek.cite}
  <div
    class="peek"
    data-below={peek.below || undefined}
    role="tooltip"
    style:left="{peek.x}px"
    style:top="{peek.y}px"
    style:inline-size="{peek.width}px"
  >
    <p class="quote">“{cite.excerpt}”</p>
    <p class="where">
      {inert
        ? m.assist_cite_gone()
        : missed === cite.n
          ? m.assist_cite_missed()
          : m.assist_cite_where({ host: cite.host })}
    </p>
  </div>
{/if}

<style>
  /* The sidebar around it is not selectable; the words to read and copy are. */
  .text {
    /* stylelint-disable-next-line property-no-vendor-prefix */
    -webkit-user-select: text;
    user-select: text;
    color: var(--color-text);
    font-size: var(--text-page-title);
    letter-spacing: -0.003em;
    line-height: 22px;
    overflow-wrap: break-word;
    text-wrap: pretty;
  }

  .text :global(:is(p, ul, ol, pre, blockquote)) {
    margin: 0 0 12px;
  }

  .text > :global(:last-child),
  .text :global(li > :last-child) {
    margin-block-end: 0;
  }

  .heading {
    margin-block-start: 16px;
    font-weight: 620;
    letter-spacing: -0.01em;
  }

  .text > .heading:first-child {
    margin-block-start: 0;
  }

  ul,
  ol {
    padding-inline-start: 18px;
  }

  li {
    padding-inline-start: 4px;
  }

  li + li {
    margin-block-start: 6px;
  }

  li :global(:is(ul, ol)) {
    margin-block: 6px 0;
  }

  li::marker {
    color: var(--color-faint);
  }

  ol > li::marker {
    font-variant-numeric: tabular-nums;
  }

  strong {
    font-weight: 620;
  }

  code {
    padding: 1px 5px;
    border-radius: 5px;
    background: var(--color-fill-active);
    font-family: var(--font-mono);
    font-size: 0.86em;
  }

  pre {
    overflow-x: auto;
    padding: 10px 12px;
    border-radius: var(--radius-row);
    background: var(--color-fill);
    box-shadow: inset 0 0 0 0.5px var(--color-border);
    font-size: 12.5px;
    line-height: 19px;
  }

  pre code {
    padding: 0;
    background: none;
  }

  blockquote {
    padding-inline-start: 12px;
    border-inline-start: 2px solid var(--color-border-strong);
    color: var(--color-label-secondary);
  }

  hr {
    block-size: 1px;
    margin: 14px 0;
    border: 0;
    background: var(--color-border);
  }

  a {
    color: inherit;
    text-decoration: underline;
    text-decoration-color: var(--color-border-strong);
    text-decoration-thickness: 1px;
    text-underline-offset: 2.5px;
    cursor: default;
    transition: text-decoration-color var(--motion-fast) var(--ease-out);
  }

  a:hover {
    text-decoration-color: currentcolor;
  }

  /* The words a delta just brought: they ink in rather than appear. */
  .fresh {
    animation: ink 360ms var(--ease-out) both;
  }

  @keyframes ink {
    from {
      opacity: 0.2;
    }
  }

  .glue {
    white-space: nowrap;
  }

  /* A source marker: small enough to sit in a line of text, solid enough to hit. */
  .cite {
    /* stylelint-disable-next-line property-no-vendor-prefix */
    -webkit-user-select: none;
    user-select: none;
    position: relative;
    display: inline-grid;
    place-items: center;
    box-sizing: border-box;
    min-inline-size: 17px;
    block-size: 17px;
    margin-inline: 2px 1px;
    padding: 0 4px;
    border: 0;
    border-radius: var(--radius-capsule);
    background: var(--row-active);
    color: var(--color-muted);
    font: inherit;
    font-size: 10.5px;
    font-variant-numeric: tabular-nums;
    font-weight: 620;
    line-height: 1;
    vertical-align: 0.14em;
    cursor: default;
    transition:
      background-color var(--motion-instant) var(--ease-smooth),
      color var(--motion-instant) var(--ease-smooth),
      scale var(--motion-fast) var(--ease-smooth);
  }

  .cite::after {
    position: absolute;
    inset: -5px -3px;
    content: "";
  }

  .cite:focus-visible {
    background: var(--color-lit);
    color: var(--color-on-lit);
    outline: none;
  }

  .cite:hover:not([aria-disabled]) {
    background: var(--color-lit);
    color: var(--color-on-lit);
  }

  .cite:active:not([aria-disabled]) {
    scale: 0.9;
  }

  .cite[aria-disabled] {
    opacity: 0.45;
  }

  .cite[data-missed] {
    color: var(--color-warning);
  }

  .peek {
    position: fixed;
    z-index: 4;
    display: flex;
    flex-direction: column;
    gap: 4px;
    box-sizing: border-box;
    padding: 10px 12px;
    border-radius: var(--radius-row);
    background: var(--color-float);
    box-shadow: var(--shadow-popover);
    pointer-events: none;
    transform: translateY(-100%);
    animation: peek 140ms var(--ease-out) both;
  }

  .peek[data-below] {
    transform: none;
  }

  @keyframes peek {
    from {
      opacity: 0;
    }
  }

  .peek p {
    margin: 0;
  }

  .quote {
    color: var(--color-text);
    font-size: 12.5px;
    line-height: 1.45;
  }

  .where {
    color: var(--color-muted);
    font-size: var(--text-caption);
    line-height: 15px;
  }

  @media (prefers-reduced-motion: reduce) {
    .fresh,
    .peek {
      animation: none;
    }
  }

  :global(:root[data-reduce-motion="true"]) .fresh,
  :global(:root[data-reduce-motion="true"]) .peek {
    animation: none;
  }

  @media (forced-colors: active) {
    .cite {
      border: 1px solid ButtonText;
    }
  }
</style>
