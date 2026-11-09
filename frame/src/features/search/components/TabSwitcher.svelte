<script lang="ts">
  import { tick, untrack } from "svelte";
  import { SvelteSet } from "svelte/reactivity";
  import * as m from "$shared/i18n/messages";
  import { commands, type TabSwitcherAction, type TabSwitcherState } from "$shared/ipc/bindings";
  import { favicons } from "$domain/favicons";
  import FavIcon from "$shared/ui/FavIcon";
  import { Globe02Icon } from "@hugeicons/core-free-icons";

  // Mirrors the native plate (desktop/src/overlay/geometry.rs), which is
  // sized to the cards it shows.
  const VISIBLE = 4;
  const CARD = 204;
  const GAP = 4;
  const PAD = 12;

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
  let across = $derived(Math.min(VISIBLE, Math.max(1, snapshot.entries.length)));
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
          if (change.intersectionRatio > 0.5 && visible.size < VISIBLE + 2) visible.add(index);
        }
      },
      { root: element, threshold: 0.5 },
    );
    void tick().then(() => {
      if (live && !disposed)
        element.querySelectorAll("button").forEach((button) => observer.observe(button));
    });
    return () => {
      live = false;
      observer.disconnect();
    };
  });
  $effect(() => {
    const selected = snapshot.selected;
    void tick().then(() => {
      if (!disposed)
        row
          ?.querySelector(`#switcher-${selected}`)
          ?.scrollIntoView({ block: "nearest", inline: "nearest", behavior: "instant" });
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
    else if (["ArrowLeft", "ArrowUp", "ArrowRight", "ArrowDown"].includes(event.key))
      intent = { type: "step", reverse: event.key === "ArrowLeft" || event.key === "ArrowUp" };
    else if (event.key === "Escape") intent = { type: "cancel" };
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
</script>

<svelte:window onkeydown={keydown} onkeyup={keyup} />
<section aria-label={m.tab_switcher_title()} data-layout={detached ? "detached" : "fused"}>
  <div
    bind:this={row}
    class="cards"
    style:inline-size={detached ? `${2 * PAD + across * CARD + (across - 1) * GAP}px` : undefined}
    role="listbox"
    tabindex="0"
    aria-label={m.tab_switcher_title()}
    aria-activedescendant={`switcher-${snapshot.selected}`}
  >
    {#each snapshot.entries as entry, index (entry.tab_id)}
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
              ><FavIcon
                size={28}
                image={favicons.image(entry.favicon)}
                tone={favicons.tone(entry.favicon)}
                fallback={Globe02Icon}
              /></span
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
    {/each}
  </div>
  {#if failed && !frozen}<p role="alert">{m.panel_action_failed()}</p>{/if}
</section>

<style>
  /* One plate, as wide as the cards on it: each card the top of its page as
     it was last seen, landscape like the page itself, its name under it. */
  section {
    display: grid;
    box-sizing: border-box;
    block-size: 100vh;
    place-items: center;
    color: var(--color-text);
    /* stylelint-disable-next-line property-no-vendor-prefix */
    -webkit-user-select: none;
    user-select: none;
  }

  .cards {
    display: flex;
    box-sizing: border-box;
    max-inline-size: 100%;
    block-size: 180px;
    align-items: center;
    overflow-x: auto;
    gap: 4px;
    padding: 12px;
    outline: none;
    scroll-padding-inline: 12px;
    scrollbar-width: none;
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
    justify-content: safe center;
    background: color-mix(in srgb, var(--color-chrome) var(--wash-launcher), transparent);
    box-shadow: inset 0 0 0 1px var(--color-border-strong);
  }

  :global(:root:is([data-material="acrylic"], [data-material="mica"])) .cards {
    box-shadow: none;
  }

  /* A card is a plate the selection fills; the page sits inside it with the
     card's corner less its inset, and the card's with the plate's less its
     padding. */
  .card {
    display: flex;
    flex: 0 0 204px;
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

  img {
    display: block;
    inline-size: 100%;
    block-size: 100%;
    object-fit: cover;
    object-position: top center;
  }

  .blank {
    display: grid;
    block-size: 100%;
    place-items: center;
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
    .card {
      transition: none;
    }
  }
</style>
