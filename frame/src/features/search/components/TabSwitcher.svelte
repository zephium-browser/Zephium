<script lang="ts">
  import { tick, untrack } from "svelte";
  import { SvelteSet } from "svelte/reactivity";
  import * as m from "$shared/i18n/messages";
  import { commands, type TabSwitcherAction, type TabSwitcherState } from "$shared/ipc/bindings";
  import { favicons } from "$domain/favicons";
  import FavIcon from "$shared/ui/FavIcon";
  import Icon from "$shared/ui/Icon";
  import { Add01Icon, Cancel01Icon, Globe02Icon } from "@hugeicons/core-free-icons";

  // Mirrors the native plate (desktop/src/overlay/geometry.rs), which is
  // sized to the cards it shows: up to four across and three down, with the
  // next row peeking in when there are more.
  const COLUMNS = 4;
  const ROWS = 3;
  const CARD = 204;
  const CARD_HEIGHT = 156;
  const GAP = 4;
  const PAD = 12;
  const PEEK = 36;
  const PREVIEWS = COLUMNS * (ROWS + 1);

  let {
    snapshot,
    frozen = false,
  }: {
    snapshot: TabSwitcherState;
    /** Leaving: the last state stays on screen while the window fades, and
     *  answers nothing. */
    frozen?: boolean;
  } = $props();
  let material = $state(document.documentElement.dataset.material ?? "none");
  $effect(() => {
    const observer = new MutationObserver(
      () => (material = document.documentElement.dataset.material ?? "none"),
    );
    observer.observe(document.documentElement, { attributeFilter: ["data-material"] });
    return () => observer.disconnect();
  });
  // Native draws the plate behind the cards where it has a material of its
  // own; otherwise the window is the plate and the page tints it.
  let detached = $derived(material === "liquid_glass" || material === "vibrancy");
  // The new tab card stands after the tabs, in the grid with them.
  let count = $derived(snapshot.entries.length + 1);
  let across = $derived(Math.min(COLUMNS, count));
  let down = $derived(Math.min(ROWS, Math.ceil(count / COLUMNS)));
  let more = $derived(Math.ceil(count / COLUMNS) > ROWS);
  let plate = $derived({
    width: 2 * PAD + across * CARD + (across - 1) * GAP,
    height: 2 * PAD + down * CARD_HEIGHT + (down - 1) * GAP + (more ? GAP + PEEK : 0),
  });
  let above = $state(false);
  let below = $state(false);
  function edges() {
    if (!row) return;
    above = row.scrollTop > 1;
    below = row.scrollTop + row.clientHeight < row.scrollHeight - 1;
  }
  let failed = $state(false);
  let row = $state<HTMLElement>();
  let queue = Promise.resolve(true);
  let disposed = false;
  const visible = new SvelteSet<number>();
  let entryIds = $derived(snapshot.entries.map((entry) => entry.tab_id).join(":"));
  $effect(() => {
    const element = row;
    void entryIds;
    if (!element) return;
    let live = true;
    untrack(() => visible.clear());
    const observer = new IntersectionObserver(
      (changes) => {
        for (const change of changes) {
          const index = Number((change.target as HTMLElement).dataset.index);
          if (change.intersectionRatio <= 0.5) visible.delete(index);
        }
        for (const change of changes) {
          const index = Number((change.target as HTMLElement).dataset.index);
          if (change.intersectionRatio > 0.5 && visible.size < PREVIEWS) visible.add(index);
        }
      },
      { root: element, threshold: 0.5 },
    );
    void tick().then(() => {
      if (live && !disposed)
        element.querySelectorAll(".card[data-index]").forEach((card) => observer.observe(card));
    });
    return () => {
      live = false;
      observer.disconnect();
    };
  });
  // Scrolled by hand: scrollIntoView would also move the window's own
  // document, sliding cards off the plate native draws behind them.
  $effect(() => {
    const selected = snapshot.selected;
    void entryIds;
    void tick().then(() => {
      const card = row?.querySelector<HTMLElement>(`#switcher-${selected}`);
      if (disposed || !row || !card) return;
      const top = card.offsetTop - PAD;
      const bottom = card.offsetTop + card.offsetHeight + PAD - row.clientHeight;
      if (row.scrollTop > top) row.scrollTop = top;
      else if (row.scrollTop < bottom) row.scrollTop = bottom;
      edges();
    });
  });

  $effect(() => {
    const session = snapshot.session_id;
    untrack(() => {
      disposed = false;
      failed = false;
      void tick().then(() => {
        if (!disposed && snapshot.session_id === session) row?.focus();
      });
    });
    return () => {
      disposed = true;
    };
  });

  function send(intent: TabSwitcherAction) {
    const session = snapshot.session_id;
    queue = queue.then(async () => {
      if (disposed || session !== snapshot.session_id) return false;
      try {
        const accepted = await commands.tabSwitcherAction(session, intent);
        if (!disposed && session === snapshot.session_id) failed = !accepted;
        return accepted;
      } catch {
        if (!disposed && session === snapshot.session_id) failed = true;
        return false;
      }
    });
    return queue;
  }

  function keydown(event: KeyboardEvent) {
    if (frozen || event.isComposing) return;
    let intent: TabSwitcherAction | undefined;
    if (event.key === "Tab") intent = { type: "step", reverse: event.shiftKey };
    else if (event.key === "ArrowLeft" || event.key === "ArrowRight")
      intent = { type: "step", reverse: event.key === "ArrowLeft" };
    else if (event.key === "ArrowUp" || event.key === "ArrowDown") {
      const index = snapshot.selected + (event.key === "ArrowUp" ? -across : across);
      if (index >= 0 && index < snapshot.entries.length) intent = { type: "select", index };
      else intent = { type: "step", reverse: event.key === "ArrowUp" };
    } else if (event.key === "Escape") intent = { type: "cancel" };
    else if (event.key === "Enter") intent = { type: "commit" };
    else if (event.ctrlKey && event.key.toLowerCase() === "w") intent = { type: "close" };
    if (intent) {
      event.preventDefault();
      event.stopImmediatePropagation();
      void send(intent);
    }
  }
  function keyup(event: KeyboardEvent) {
    if (!frozen && event.key === "Control" && snapshot.keyboard) void send({ type: "commit" });
  }
  async function choose(index: number) {
    if (frozen) return;
    if (await send({ type: "select", index })) await send({ type: "commit" });
  }
  function newTab() {
    if (!frozen) void send({ type: "new_tab" });
  }
  async function closeAt(index: number) {
    if (frozen) return;
    if (await send({ type: "select", index })) await send({ type: "close" });
  }
</script>

<svelte:window onkeydown={keydown} onkeyup={keyup} />
<section aria-label={m.tab_switcher_title()} data-layout={detached ? "detached" : "fused"}>
  <div
    bind:this={row}
    class="cards"
    style:inline-size={detached ? `${plate.width}px` : undefined}
    style:block-size={detached ? `${plate.height}px` : undefined}
    style:--across={across}
    data-above={above || undefined}
    data-below={below || undefined}
    onscroll={edges}
    role="listbox"
    tabindex="0"
    aria-label={m.tab_switcher_title()}
    aria-activedescendant={`switcher-${snapshot.selected}`}
  >
    {#each snapshot.entries as entry, index (entry.tab_id)}
      <div class="slot">
        <button
          id={`switcher-${index}`}
          class="card"
          data-index={index}
          type="button"
          role="option"
          tabindex="-1"
          aria-selected={index === snapshot.selected}
          aria-label={`${entry.title || entry.url_host} ${entry.url_host}`}
          onclick={() => void choose(index)}
        >
          <span class="page">
            {#if entry.preview_url && visible.has(index)}<img
                src={entry.preview_url}
                alt=""
                draggable="false"
              />{:else}<span class="blank"
                ><span class="badge"
                  ><FavIcon
                    size={24}
                    image={favicons.image(entry.favicon)}
                    tone={favicons.tone(entry.favicon)}
                    fallback={Globe02Icon}
                  /></span
                ><span class="host">{entry.url_host}</span></span
              >{/if}
          </span>
          <span class="caption">
            <FavIcon
              size={14}
              image={favicons.image(entry.favicon)}
              tone={favicons.tone(entry.favicon)}
              fallback={Globe02Icon}
            />
            <span class="title">{entry.title || entry.url_host}</span>
          </span>
        </button>
        <button
          class="close"
          type="button"
          tabindex="-1"
          aria-label={m.close_tab()}
          onclick={() => void closeAt(index)}
          ><Icon icon={Cancel01Icon} size={12} strokeWidth={2} /></button
        >
      </div>
    {/each}
    <div class="slot">
      <button type="button" class="card new" tabindex="-1" onclick={newTab}>
        <span class="page"><span class="plus"><Icon icon={Add01Icon} size={22} /></span></span>
        <span class="caption"><span class="title">{m.new_tab()}</span></span>
      </button>
    </div>
  </div>
  {#if failed && !frozen}<p role="alert">{m.panel_action_failed()}</p>{/if}
</section>

<style>
  /* One plate, as wide as the cards on it: each card the top of its page as
     it was last seen, landscape like the page itself, its name under it. */
  section {
    position: fixed;
    inset: 0;
    display: grid;
    box-sizing: border-box;
    overflow: hidden;
    place-items: center;
    color: var(--color-text);
    /* stylelint-disable-next-line property-no-vendor-prefix */
    -webkit-user-select: none;
    user-select: none;
  }

  /* Past the last full row the next one peeks in under a fade, and the top
     fades too once rows have scrolled away above. */
  .cards {
    display: grid;
    box-sizing: border-box;
    grid-template-columns: repeat(var(--across), 204px);
    align-content: start;
    max-inline-size: 100%;
    max-block-size: 100%;
    overflow: hidden auto;
    gap: 4px;
    padding: 12px;
    outline: none;
    overscroll-behavior: contain;
    scrollbar-width: none;
  }

  .cards[data-below] {
    mask-image: var(--mask-fade-below);
  }

  .cards[data-above] {
    mask-image: var(--mask-fade-above);
  }

  .cards[data-above][data-below] {
    mask-image: var(--mask-fade-ends);
  }

  .cards::-webkit-scrollbar {
    display: none;
  }

  /* Without a native plate the window is the plate: the page tints all of it,
     and the system draws its rim. */
  [data-layout="fused"] {
    place-items: stretch;
  }

  [data-layout="fused"] .cards {
    inline-size: 100%;
    block-size: 100%;
    place-content: safe center;
    background: color-mix(in srgb, var(--color-chrome) var(--wash-launcher), transparent);
    box-shadow: inset 0 0 0 1px var(--color-border-strong);
  }

  :global(:root:is([data-material="acrylic"], [data-material="mica"])) .cards {
    box-shadow: none;
  }

  /* Each card stands in a slot that also holds its close control, which
     shows over the page's corner while the pointer is on the card. */

  /* Fixed, whatever the title: a long one ellipses inside its card. */
  .slot {
    position: relative;
    min-inline-size: 0;
    inline-size: 204px;
  }

  .close {
    position: absolute;
    inset-block-start: 12px;
    inset-inline-end: 12px;
    display: grid;
    place-items: center;
    inline-size: 22px;
    block-size: 22px;
    padding: 0;
    border: 0;
    border-radius: 50%;
    background: color-mix(in srgb, var(--color-float) 86%, transparent);
    box-shadow: var(--shadow-raised);
    color: var(--color-muted);
    opacity: 0;
    scale: 0.85;
    backdrop-filter: blur(12px);
    transition:
      opacity var(--motion-instant) var(--ease-smooth),
      scale var(--motion-instant) var(--ease-smooth),
      color var(--motion-instant) var(--ease-smooth);
  }

  .close:hover {
    color: var(--color-text);
  }

  .slot:hover .close {
    opacity: 1;
    scale: 1;
  }

  /* A card is a plate the selection fills; the page sits inside it with the
     card's corner less its inset, and the card's with the plate's less its
     padding. */
  .card {
    display: flex;
    inline-size: 100%;
    flex-direction: column;
    box-sizing: border-box;
    min-inline-size: 0;
    gap: 8px;
    padding: 6px;
    border: 0;
    border-radius: var(--radius-control);
    background: transparent;
    color: var(--color-muted);
    transition:
      background-color var(--motion-instant) var(--ease-smooth),
      color var(--motion-instant) var(--ease-smooth);
  }

  .card[aria-selected="true"] {
    background: color-mix(in srgb, var(--color-text) 11%, transparent);
    color: var(--color-text);
  }

  .page {
    position: relative;
    display: block;
    overflow: hidden;
    block-size: 120px;
    border-radius: var(--radius-inset);
    background: var(--color-tile);
    box-shadow: var(--shadow-raised);
  }

  /* The new tab card: an empty page with a plus, quieter than the tabs. */
  .new .page {
    display: grid;
    place-items: center;
    background: transparent;
    box-shadow: inset 0 0 0 1px var(--color-border-strong);
  }

  .plus {
    display: grid;
    place-items: center;
    inline-size: 44px;
    block-size: 44px;
    border-radius: var(--radius-capsule);
    background: color-mix(in srgb, var(--color-text) 7%, transparent);
    color: var(--color-muted);
    transition:
      background-color var(--motion-instant) var(--ease-smooth),
      color var(--motion-instant) var(--ease-smooth);
  }

  .new:hover .plus {
    background: color-mix(in srgb, var(--color-text) 12%, transparent);
    color: var(--color-text);
  }

  img {
    display: block;
    inline-size: 100%;
    block-size: 100%;
    object-fit: cover;
    object-position: top center;
  }

  /* A tab not seen since it slept has no picture: its mark on a raised
     badge and the site it is on stand in for one. */
  .blank {
    display: flex;
    block-size: 100%;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 10px;
    padding-inline: 12px;
    background: linear-gradient(
      to bottom,
      color-mix(in srgb, var(--color-text) 4%, transparent),
      transparent
    );
  }

  .badge {
    display: grid;
    place-items: center;
    inline-size: 44px;
    block-size: 44px;
    border-radius: var(--radius-row);
    background: var(--color-float);
    box-shadow: var(--shadow-raised);
  }

  .host {
    max-inline-size: 100%;
    overflow: hidden;
    color: var(--color-faint);
    font-size: 11px;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .caption {
    display: flex;
    align-items: center;
    gap: 6px;
    min-inline-size: 0;
    block-size: 16px;
    padding-inline: 2px;
    font-size: var(--text-label);
    font-weight: 500;
    line-height: 16px;
  }

  .title {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  p {
    position: absolute;
    inset-block-end: 4px;
    margin: 0;
    color: var(--color-danger);
    font-size: 12px;
  }

  @media (prefers-reduced-motion: reduce) {
    .card,
    .close {
      transition: none;
    }
  }
</style>
