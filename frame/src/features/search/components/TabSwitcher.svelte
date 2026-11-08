<script lang="ts">
  import { tick, untrack } from "svelte";
  import { SvelteSet } from "svelte/reactivity";
  import * as m from "$shared/i18n/messages";
  import { commands, type TabSwitcherAction, type TabSwitcherState } from "$shared/ipc/bindings";
  import { favicons } from "$domain/favicons";
  import FavIcon from "$shared/ui/FavIcon";
  import { Globe02Icon } from "@hugeicons/core-free-icons";

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
          if (change.intersectionRatio > 0.5 && visible.size < 7) visible.add(index);
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
    class:centred={snapshot.entries.length <= 7}
    role="listbox"
    tabindex="0"
    aria-label={m.tab_switcher_title()}
    aria-activedescendant={`switcher-${snapshot.selected}`}
  >
    {#each snapshot.entries as entry, index (entry.tab_id)}
      <button
        id={`switcher-${index}`}
        data-index={index}
        type="button"
        role="option"
        tabindex="-1"
        aria-selected={index === snapshot.selected}
        aria-label={`${entry.title || entry.url_host} ${entry.url_host}`}
        onclick={() => void choose(index)}
      >
        {#if entry.preview_url && visible.has(index)}<img
            src={entry.preview_url}
            alt=""
            draggable="false"
          />{:else}<span class="preview"></span>{/if}
        <FavIcon
          image={favicons.image(entry.favicon)}
          tone={favicons.tone(entry.favicon)}
          fallback={Globe02Icon}
        />
        <span class="title">{entry.title || entry.url_host}</span>
      </button>
    {/each}
  </div>
  {#if failed}<p role="alert">{m.panel_action_failed()}</p>{/if}
</section>

<style>
  section {
    padding: 40px;
    color: var(--color-text);
  }

  .cards {
    display: flex;
    overflow-x: auto;
    gap: 8px;
    padding: 12px;
    background: var(--color-chrome);
    border-radius: var(--panel-radius);
  }

  .cards.centred {
    justify-content: center;
  }

  button {
    flex: 0 0 calc((100% - 48px) / 7);
    box-sizing: border-box;
    min-width: 0;
    padding: 8px;
    color: var(--color-text);
    background: transparent;
    border: 1px solid var(--color-border);
    border-radius: var(--radius-row);
  }

  button[aria-selected="true"] {
    outline: 2px solid var(--color-ring);
  }

  img,
  .preview {
    display: block;
    width: 100%;
    height: 120px;
    object-fit: contain;
  }

  .title {
    display: block;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
