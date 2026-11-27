<script lang="ts">
  import { Globe02Icon } from "@hugeicons/core-free-icons";
  import { tabs } from "$domain/tabs";
  import { favicons } from "$domain/favicons";
  import { commands, type ClosedTabView } from "$shared/ipc/bindings";
  import { observe } from "$shared/lib/observe";
  import * as m from "$shared/i18n/messages";
  import FavIcon from "$shared/ui/FavIcon";

  let { profile }: { profile: string } = $props();

  const SHOWN = 5;
  const DEADLINE_MS = 4000;

  let closed = $state.raw<ClosedTabView[]>([]);
  let expanded = $state(false);
  const rows = $derived(expanded ? closed : closed.slice(0, SHOWN));

  // What closes or comes back while the panel is open changes the front tab or
  // the count, so those are what ask again.
  $effect(() => {
    const owner = profile;
    void tabs.activeId();
    void tabs.tabs().length;
    const request = new AbortController();
    void (async () => {
      const seen = await observe(
        commands.historyCall(owner, { kind: "closed_tabs" }),
        DEADLINE_MS,
        request.signal,
      );
      if (seen.state === "received" && seen.value.kind === "closed_tabs") {
        closed = seen.value.tabs;
      }
    })();
    return () => request.abort();
  });

  const reopen = (id: string) => commands.historyCall(profile, { kind: "reopen_closed", id });
</script>

{#if closed.length > 0}
  <section class="closed" aria-labelledby="closed-title">
    <h2 id="closed-title">{m.history_recently_closed()}</h2>
    <ul>
      {#each rows as tab (tab.id)}
        {@const site = favicons.mark(tab.icon, tab.url)}
        <li>
          <button type="button" onclick={() => void reopen(tab.id)}>
            <FavIcon
              image={site?.image ?? null}
              tone={site?.tone}
              size={16}
              lit
              fallback={Globe02Icon}
            />
            <span>{tab.title || tab.url}</span>
          </button>
        </li>
      {/each}
    </ul>
    {#if closed.length > SHOWN}
      <button type="button" class="toggle" onclick={() => (expanded = !expanded)}
        >{expanded ? m.history_closed_fewer() : m.history_more()}</button
      >
    {/if}
  </section>
{/if}

<style>
  .closed {
    flex: none;
    padding: 8px 8px 4px;
    border-block-end: 1px solid var(--color-border);
  }

  h2 {
    margin: 0;
    padding: 0 10px 6px;
    font-size: var(--text-label);
    font-weight: 550;
    letter-spacing: 0.01em;
    color: var(--color-muted);
  }

  ul {
    margin: 0;
    padding: 0;
    list-style: none;
  }

  li button {
    display: grid;
    grid-template-columns: 16px minmax(0, 1fr);
    align-items: center;
    gap: 10px;
    box-sizing: border-box;
    width: 100%;
    block-size: 34px;
    padding: 0 10px;
    border: 0;
    border-radius: var(--radius-row);
    background: none;
    color: var(--color-text);
    font: inherit;
    font-size: var(--text-label);
    text-align: start;
    cursor: default;
    transition: background-color var(--motion-instant) var(--ease-smooth);
  }

  li button:hover,
  li button:focus-visible {
    background: var(--row-active);
  }

  li span {
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .toggle {
    display: block;
    width: 100%;
    margin: 2px 0 4px;
    padding: 6px 10px;
    border: 0;
    border-radius: var(--radius-row);
    background: none;
    color: var(--color-muted);
    font-size: var(--text-label);
    text-align: center;
    cursor: default;
  }

  .toggle:hover {
    background: var(--color-fill-hover);
  }

  @media (prefers-reduced-motion: reduce) {
    li button {
      transition: none;
    }
  }

  @media (forced-colors: active) {
    li button:focus-visible {
      outline: 1px solid Highlight;
    }
  }
</style>
