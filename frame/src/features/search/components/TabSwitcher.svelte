<script lang="ts">
  import { tick, untrack } from "svelte";
  import { SvelteSet } from "svelte/reactivity";
  import * as m from "$shared/i18n/messages";
  import { commands, type TabSwitcherAction, type TabSwitcherState } from "$shared/ipc/bindings";
  import { favicons } from "$domain/favicons";
  import FavIcon from "$shared/ui/FavIcon";
  import { Globe02Icon } from "@hugeicons/core-free-icons";

  const VISIBLE = 5;

  let { snapshot }: { snapshot: TabSwitcherState } = $props();
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
    if (event.isComposing) return;
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
    if (event.key === "Control" && snapshot.keyboard) void send({ type: "commit" });
  }
  async function choose(index: number) {
    if (await send({ type: "select", index })) await send({ type: "commit" });
  }
</script>

<svelte:window onkeydown={keydown} onkeyup={keyup} />
<section aria-label={m.tab_switcher_title()}>
  <div
    bind:this={row}
    class="cards"
    class:centred={snapshot.entries.length <= VISIBLE}
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
  {#if failed}<p role="alert">{m.panel_action_failed()}</p>{/if}
</section>

<style>
  /* The switcher fills the launcher's sheet: five tabs across, each the top
     of its page as it was last seen, its name under it. */
  section {
    box-sizing: border-box;
    block-size: 100vh;
    padding: 40px;
    color: var(--color-text);
    /* stylelint-disable-next-line property-no-vendor-prefix */
    -webkit-user-select: none;
    user-select: none;
  }

  .cards {
    --gap: 6px;

    display: flex;
    box-sizing: border-box;
    block-size: 100%;
    align-items: center;
    overflow-x: auto;
    gap: var(--gap);
    padding-inline: 10px;
    border-radius: var(--panel-radius, var(--radius-panel));
    background: color-mix(in srgb, var(--color-chrome) var(--wash-launcher), transparent);
    box-shadow: inset 0 0 0 1px var(--color-border-strong);
    outline: none;
    scrollbar-width: none;
  }

  :global(:root:is([data-material="acrylic"], [data-material="mica"])) .cards {
    box-shadow: none;
  }

  .cards.centred {
    justify-content: center;
  }

  /* A card is a plate the selection fills; the page sits inside it with the
     plate's corner less its inset. */
  .card {
    --inset: 6px;

    display: flex;
    flex: 0 0 calc((100% - 4 * var(--gap)) / 5);
    flex-direction: column;
    box-sizing: border-box;
    min-inline-size: 0;
    gap: 8px;
    padding: var(--inset);
    border: 0;
    border-radius: var(--radius-card);
    background: transparent;
    color: var(--color-muted);
    transition:
      background-color var(--motion-instant) var(--ease-smooth),
      color var(--motion-instant) var(--ease-smooth);
  }

  .card[aria-selected="true"] {
    background: color-mix(in srgb, var(--color-text) 9%, transparent);
    color: var(--color-text);
  }

  .page {
    position: relative;
    display: block;
    overflow: hidden;
    aspect-ratio: 4 / 5;
    border-radius: calc(var(--radius-card) - var(--inset));
    background: var(--color-tile);
    box-shadow: inset 0 0 0 0.5px var(--color-border);
  }

  .card[aria-selected="true"] .page {
    box-shadow: inset 0 0 0 1px var(--color-border-strong);
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

  @media (prefers-reduced-motion: reduce) {
    .card {
      transition: none;
    }
  }
</style>
