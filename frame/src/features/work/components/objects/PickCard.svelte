<script lang="ts">
  import Icon from "$shared/ui/Icon";
  import * as m from "$shared/i18n/messages";
  import type { ObjectActions, PickView } from "../../lib/board/types";
  import Mark, { hasMark } from "./Mark.svelte";
  import YesNo from "./YesNo.svelte";
  import { Tick02Icon } from "./icons";
  import Star from "./Star.svelte";
  import Stars from "./Stars.svelte";
  import { pickDetails } from "../../lib/board/rating";
  import { unrepeated } from "../../lib/board/text";
  import PlayMark from "./PlayMark.svelte";
  import { youtubeId } from "../../lib/link-media";
  import { thumbnail } from "$shared/canvas";
  import { getContext } from "svelte";
  import { canvasFar } from "../../lib/canvas-context";
  /**
   * One thing to choose: its own picture first, else its mark beside its name,
   * else its words alone. Facts are typed, the price is the loudest figure.
   */
  let {
    pick,
    video = false,
    product = false,
    actions = {},
    onopen,
  }: {
    pick: PickView;
    /** A video: the picture is its poster, with its length and a play mark. */
    video?: boolean;
    /** A product: its photo is shown whole on a plate, never cropped. */
    product?: boolean;
    actions?: ObjectActions;
    onopen?: () => void;
  } = $props();
  const tube = $derived(video && !!pick.url && !!youtubeId(pick.url));
  const figureOf = (text: string) => Number(text.replace(/[^\d.]/gu, "")) || 0;
  const display = $derived(unrepeated(pick.price?.display ?? ""));
  /**
   * The price before a discount: the price's own, else a fact that says what
   * it was ("Original monthly $2,632") when it is higher than the price now.
   */
  const earlier = $derived.by(() => {
    if (!pick.price) return null;
    if (pick.price.was) return { value: pick.price.was, fact: null };
    const now = pick.price.amount ?? figureOf(display);
    const fact = pick.facts.find(
      (entry) =>
        /^(original|was|regular|usual|list|before)\b/iu.test(entry.label.trim()) &&
        figureOf(entry.value) > now &&
        now > 0,
    );
    return fact ? { value: fact.value, fact } : null;
  });
  /** A rating the page worded as a fact is stars, not a row; a fact that repeats the name is no fact. */
  const details = $derived(pickDetails(pick));
  const facts = $derived(details.facts.filter((entry) => entry !== earlier?.fact));
  /** "Top pick" says it once: a tag that repeats it goes. */
  const tags = $derived(
    pick.recommended
      ? pick.tags.filter((tag) => !/^(recommended|top pick)$/iu.test(tag.trim()))
      : pick.tags,
  );
  /** The address that would not load; a new picture gets its own chance. */
  let broken = $state<string | null>(null);
  /** Seen from far, a smaller copy of the photo is enough. */
  const scale = getContext<{ readonly far: boolean } | undefined>(canvasFar);
  const photoWidth = $derived(scale?.far ? 240 : 640);
  const failed = $derived(!!pick.picture && broken === pick.picture.src);
  const picture = $derived(pick.picture && !failed ? pick.picture : null);
  const logo = $derived(!picture && pick.logo && hasMark(pick.logo) ? pick.logo : null);
  /** "$4,350 / month": the figure loud, its period quiet. */
  const price = $derived.by(() => {
    const at = display.search(/\s*(\/|per\b)/u);
    return at > 0
      ? { figure: display.slice(0, at), period: display.slice(at) }
      : { figure: display, period: "" };
  });
  /** The name's longest word, so a single long word shrinks the title rather than clipping. */
  const word = $derived(
    pick.name.split(/\s+/u).reduce((a, b) => (b.length > a.length ? b : a), ""),
  );
  const longest = $derived(Math.max(6, word.length));
  let room = $state(0);
  /**
   * Whole words first: the name shrinks (to its floor) so its longest word fits.
   * Only a word that cannot fit even there breaks, with a hyphen, and never one
   * a person reads as a name.
   */
  const hyphenate = $derived(
    !!room && longest * 0.64 * 12 > room && word[0] === word[0]?.toLowerCase(),
  );
</script>

<article
  class="pick"
  class:recommended={pick.recommended}
  class:chosen={pick.chosen}
  class:pictured={!!picture}
>
  {#if picture}
    <div class="photo" class:video class:product>
      <!-- The photo at the size its card shows it: the full-size picture isn't kept. -->
      <canvas
        data-picture={picture.src}
        use:thumbnail={{
          url: picture.src,
          width: photoWidth,
          onmissing: () => (broken = pick.picture?.src ?? null),
        }}
      ></canvas>
      {#if video}<span class="play" aria-hidden="true"
          ><PlayMark size={tube ? 52 : 44} youtube={tube} /></span
        >{#if pick.duration}<span class="length">{pick.duration}</span>{/if}{/if}
      {#if pick.recommended}<span class="badge"><Star size={11} />{m.work_pick_top()}</span>{/if}
    </div>
  {/if}
  <div class="body">
    {#if !picture && pick.recommended}<span class="flag"><Star size={11} />{m.work_pick_top()}</span
      >{/if}
    <header>
      {#if logo}<Mark address={logo} size={20} />{/if}
      <div class="names" bind:clientWidth={room}>
        <h4 style:--longest={longest} class:hyphenate title={pick.name}>
          {#if pick.url}<a
              class="nodrag"
              href={pick.url}
              onclick={(event) => {
                if (!actions.link) return;
                event.preventDefault();
                actions.link(pick.url!);
              }}>{pick.name}</a
            >{:else}{pick.name}{/if}
        </h4>
        {#if pick.subtitle}<p class="subtitle">
            {pick.subtitle}
          </p>{/if}
      </div>
    </header>
    {#if details.rating || (video && pick.duration && !picture)}
      <p class="meta">
        {#if details.rating}<Stars rating={details.rating} />{/if}
        {#if video && pick.duration && !picture}<span>{pick.duration}</span>{/if}
      </p>
    {/if}
    {#if facts.length}
      <dl class="facts">
        {#each facts as fact (fact.label)}
          <div>
            <dt>{fact.label}</dt>
            <dd>
              {#if fact.kind === "yes" || fact.kind === "no" || fact.kind === "partial"}<YesNo
                  value={fact.kind}
                  size={14}
                />{/if}{#if !/^(yes|no)$/iu.test(fact.value.trim())}<span
                  class="value"
                  class:quiet={fact.kind === "no"}
                  title={fact.value}>{fact.value}</span
                >{/if}
            </dd>
          </div>
        {/each}
      </dl>
    {/if}
    {#if pick.why}<p class="why">{pick.why}</p>{/if}
    {#if tags.length}
      <ul class="tags">
        {#each tags as tag (tag)}<li>{tag}</li>{/each}
      </ul>
    {/if}
    {#if pick.price}
      <p class="price">
        <span class="figure">{price.figure}</span>{#if price.period}<span class="period"
            >{price.period}</span
          >{/if}{#if earlier}<s class="was" aria-label={m.work_pick_was({ price: earlier.value })}
            >{earlier.value}</s
          >{/if}
      </p>
    {/if}
  </div>
  {#if actions.choose || actions.ask || onopen}
    <div class="actions">
      {#if actions.choose && pick.element}<button
          type="button"
          class="nodrag nopan"
          class:on={pick.chosen}
          onclick={() => actions.choose?.(pick.element!, !pick.chosen)}
          >{#if pick.chosen}<Icon
              icon={Tick02Icon}
              size={13}
            />{m.work_env_decided()}{:else}{m.work_env_choose()}{/if}</button
        >{/if}
      {#if actions.ask}<button
          type="button"
          class="nodrag nopan"
          onclick={() => actions.ask?.(pick.name)}>{m.work_env_ask()}</button
        >{/if}
      {#if onopen}<button type="button" class="nodrag nopan" onclick={onopen}
          >{m.work_env_open()}</button
        >{/if}
    </div>
  {/if}
</article>

<style>
  .pick {
    position: relative;
    display: flex;
    flex-direction: column;
    box-sizing: border-box;
    min-inline-size: 0;
    overflow: hidden;
    border-radius: var(--radius-card);
    background: var(--color-surface);
    box-shadow: var(--shadow-raised);
    color: var(--color-text);
  }

  .pick.recommended {
    box-shadow:
      0 0 0 1.5px var(--color-border-strong),
      var(--shadow-raised);
  }

  .pick.chosen {
    box-shadow:
      0 0 0 2px var(--color-success),
      var(--shadow-raised);
  }

  .photo {
    position: relative;
    aspect-ratio: 4 / 3;
    overflow: hidden;
    background: var(--color-fill);
  }

  .photo.video {
    aspect-ratio: 16 / 9;
  }

  .photo canvas {
    display: block;
    inline-size: 100%;
    block-size: 100%;
    object-fit: cover;
  }

  /* A product shot on white: whole, on a plate its white melts into. */
  .photo.product {
    padding: 12px;
    background: var(--color-product-plate);
  }

  .photo.product canvas {
    object-fit: contain;
    mix-blend-mode: multiply;
  }

  .play {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    color: var(--color-on-lit);
  }

  .length,
  .badge {
    position: absolute;
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 2px 8px;
    border-radius: var(--radius-capsule);
    background: var(--color-float);
    color: var(--color-text);
    font-size: var(--text-caption);
    font-weight: 600;
    font-variant-numeric: tabular-nums;
  }

  .length {
    inset-block-end: 8px;
    inset-inline-end: 8px;
  }

  .badge {
    inset-block-start: 10px;
    inset-inline-start: 10px;
  }

  .body {
    display: flex;
    flex: 1;
    flex-direction: column;
    gap: 10px;
    padding: 14px 16px 16px;
  }

  .flag {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    color: var(--color-label-secondary);
    font-size: var(--text-caption);
    font-weight: 600;
  }

  header {
    display: flex;
    align-items: flex-start;
    gap: 10px;
  }

  header :global(.mark) {
    margin-block-start: 1px;
  }

  .names {
    display: flex;
    flex: 1;
    flex-direction: column;
    gap: 3px;
    min-inline-size: 0;
    container-type: inline-size;
  }

  h4 {
    margin: 0;
    font-size: clamp(12px, 100cqi / (var(--longest) * 0.64), var(--text-page-title));
    font-weight: 600;
    line-height: 19px;
    letter-spacing: -0.005em;
    text-wrap: pretty;

    /* Extracted names stay whole in the data; the card shows three lines. */
    display: -webkit-box;
    overflow: hidden;
    -webkit-box-orient: vertical;
    -webkit-line-clamp: 3;
    line-clamp: 3;
  }

  h4.hyphenate {
    hyphens: auto;
  }

  h4 a {
    color: inherit;
    text-decoration: none;
  }

  h4 a:hover {
    text-decoration: underline;
    text-decoration-color: var(--color-border-strong);
    text-underline-offset: 3px;
  }

  .subtitle,
  .meta {
    margin: 0;
    color: var(--color-muted);
    font-size: var(--text-label);
    line-height: 16px;
  }

  .meta {
    display: flex;
    gap: 10px;
  }

  .facts {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin: 0;
    font-size: var(--text-label);
  }

  .facts div {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 12px;
  }

  dt {
    flex: none;
    color: var(--color-muted);
  }

  dd {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    margin: 0;
    min-inline-size: 0;
    text-align: end;
  }

  dd .value {
    display: -webkit-box;
    overflow: hidden;
    -webkit-box-orient: vertical;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    overflow-wrap: anywhere;
  }

  dd :global(.mark) {
    align-self: center;
  }

  .quiet {
    color: var(--color-muted);
  }

  .why {
    margin: 0;
    color: var(--color-label-secondary);
    font-size: var(--text-label);
    line-height: 17px;
    text-wrap: pretty;
  }

  .tags {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .tags li {
    padding: 1px 8px;
    border-radius: var(--radius-capsule);
    background: var(--color-fill);
    color: var(--color-label-secondary);
    font-size: var(--text-caption);
    line-height: 16px;
  }

  .price {
    display: flex;
    align-items: baseline;
    gap: 4px;
    margin: auto 0 0;
    padding-block-start: 2px;
  }

  .figure {
    font-size: calc(var(--text-page-title) + 3px);
    font-weight: 650;
    font-variant-numeric: tabular-nums;
    letter-spacing: -0.01em;
  }

  .period {
    color: var(--color-muted);
    font-size: var(--text-label);
  }

  /* What it cost before: beside the price, quiet and struck, never a second figure. */
  .was {
    margin-inline-start: 4px;
    color: var(--color-faint);
    font-size: var(--text-label);
    font-variant-numeric: tabular-nums;
    text-decoration-thickness: 1px;
  }

  .actions {
    position: absolute;
    inset-block-start: 10px;
    inset-inline-end: 10px;
    display: flex;
    gap: 4px;
    opacity: 0;
    transition: opacity var(--motion-fast) var(--ease-out);
  }

  .pick:hover .actions,
  .pick:focus-within .actions {
    opacity: 1;
  }

  .actions button {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    block-size: 26px;
    padding: 0 10px;
    border: 0;
    border-radius: var(--radius-capsule);
    background: var(--color-float);
    box-shadow: var(--shadow-float);
    color: var(--color-text);
    font: inherit;
    font-size: var(--text-label);
    font-weight: 550;
    cursor: default;
  }

  .actions button:hover {
    background: var(--color-lit);
    color: var(--color-on-lit);
  }

  .actions button.on {
    color: var(--color-success);
  }
</style>
