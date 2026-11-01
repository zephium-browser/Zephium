<!--
  Obsidian's right sidebar: the open file's outline, the notes that link to
  it, and a month calendar whose days open the daily journal note.
-->
<script lang="ts">
  import { commands, type NoteSummary } from "$shared/ipc/bindings";
  import type { VaultSession } from "$domain/vault";
  import * as m from "$shared/i18n/messages";
  import type { OutlineItem } from "../lib/outline";
  import { journalPath, monthGrid } from "../lib/journal";
  import { extOf } from "../lib/kinds";

  let {
    session,
    current,
    outline,
    ongo,
    onopen,
    onjournal,
  }: {
    session: VaultSession;
    current: string | null;
    outline: OutlineItem[];
    ongo: (target: unknown) => void;
    onopen: (path: string) => void;
    onjournal: (date: Date) => void;
  } = $props();

  type Tab = "outline" | "links" | "calendar";
  let tab = $state<Tab>("outline");
  let backlinks = $state.raw<NoteSummary[]>([]);
  let month = $state(new Date());
  let grid = $derived(monthGrid(month));
  let today = new Date();
  let journals = $derived(
    new Set(session.listings.get("Journal")?.map((entry) => entry.path) ?? []),
  );

  $effect(() => {
    const path = current;
    void session.revision;
    backlinks = [];
    if (tab !== "links" || !path || extOf(path) !== "md") return;
    void (async () => {
      const id = await session.noteId(path);
      if (!id || path !== current) return;
      try {
        const reply = await commands.noteCall(session.profile, { kind: "backlinks", id });
        if (reply.response.kind === "page" && path === current) backlinks = reply.response.items;
      } catch {
        backlinks = [];
      }
    })();
  });

  $effect(() => {
    if (tab === "calendar") void session.list("Journal");
  });
</script>

<aside class="panel" aria-label={m.vault_details()}>
  <div class="switch" role="tablist">
    {#each [["outline", m.vault_outline(), "O"], ["links", m.vault_backlinks(), "B"], ["calendar", m.vault_calendar(), "C"]] as const as [id, label, tip] (id)}
      <button
        type="button"
        role="tab"
        aria-selected={tab === id}
        class="tab"
        class:on={tab === id}
        data-keytip={tip}
        onclick={() => (tab = id)}>{label}</button
      >
    {/each}
  </div>
  <div class="content">
    {#if tab === "outline"}
      {#if outline.length === 0}
        <p class="empty">{m.vault_no_headings()}</p>
      {:else}
        <ul>
          {#each outline as item, index (index)}
            <li style:--level={item.level - 1}>
              <button type="button" class="item" onclick={() => ongo(item.target)}
                >{item.title}</button
              >
            </li>
          {/each}
        </ul>
      {/if}
    {:else if tab === "links"}
      {#if !current || extOf(current) !== "md"}
        <p class="empty">{m.vault_backlinks_notes_only()}</p>
      {:else if backlinks.length === 0}
        <p class="empty">{m.vault_no_backlinks()}</p>
      {:else}
        <ul>
          {#each backlinks as note (note.id)}
            <li>
              <button type="button" class="item" onclick={() => onopen(note.path)}>
                <span>{note.title}</span>
                <small>{note.preview}</small>
              </button>
            </li>
          {/each}
        </ul>
      {/if}
    {:else}
      <div class="calendar">
        <div class="month">
          <button
            type="button"
            class="nav"
            aria-label={m.vault_previous_month()}
            onclick={() => (month = new Date(month.getFullYear(), month.getMonth() - 1, 1))}
            >‹</button
          >
          <strong>{month.toLocaleDateString(undefined, { month: "long", year: "numeric" })}</strong>
          <button
            type="button"
            class="nav"
            aria-label={m.vault_next_month()}
            onclick={() => (month = new Date(month.getFullYear(), month.getMonth() + 1, 1))}
            >›</button
          >
        </div>
        <button type="button" class="today" onclick={() => onjournal(new Date())}
          >{m.vault_today()}</button
        >
        <div class="days" role="grid" aria-label={m.vault_calendar()}>
          {#each grid.weekdays as day (day)}<span class="weekday">{day}</span>{/each}
          {#each grid.days as day (day.getTime())}
            {@const has = journals.has(journalPath(day))}
            <button
              type="button"
              class="day"
              class:outside={day.getMonth() !== month.getMonth()}
              class:today={day.toDateString() === today.toDateString()}
              class:has
              aria-label={day.toLocaleDateString()}
              onclick={() => onjournal(day)}>{day.getDate()}</button
            >
          {/each}
        </div>
      </div>
    {/if}
  </div>
</aside>

<style>
  .panel {
    display: flex;
    flex-direction: column;
    min-block-size: 0;
    border-inline-start: 1px solid var(--color-border);
    background: var(--color-surface);
    font-size: 13px;
  }

  .switch {
    display: flex;
    gap: 2px;
    padding: 6px;
    border-block-end: 1px solid var(--color-border);
  }

  .tab {
    flex: 1;
    block-size: 26px;
    border: 0;
    border-radius: var(--radius-control-compact);
    background: transparent;
    color: var(--color-muted);
    font: inherit;
    font-size: 12px;
  }

  .tab.on {
    background: var(--color-fill-active);
    color: var(--color-text);
  }

  .content {
    flex: 1;
    min-block-size: 0;
    padding: 6px;
    overflow: auto;
  }

  ul {
    margin: 0;
    padding: 0;
    list-style: none;
  }

  li {
    padding-inline-start: calc(var(--level, 0) * 12px);
  }

  .item {
    display: flex;
    flex-direction: column;
    inline-size: 100%;
    padding: 4px 8px;
    border: 0;
    border-radius: var(--radius-row);
    background: transparent;
    color: var(--color-label-secondary);
    font: inherit;
    text-align: start;
  }

  .item:hover {
    background: var(--row-hover);
    color: var(--color-text);
  }

  small {
    overflow: hidden;
    color: var(--color-faint);
    font-size: 11px;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .empty {
    padding: 16px 8px;
    color: var(--color-muted);
    text-align: center;
  }

  .month {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 4px;
  }

  .nav,
  .today {
    block-size: 26px;
    padding-inline: 10px;
    border: 0;
    border-radius: var(--radius-control-compact);
    background: transparent;
    color: var(--color-label-secondary);
    font: inherit;
  }

  .today {
    margin: 4px;
    background: var(--color-fill);
  }

  .days {
    display: grid;
    grid-template-columns: repeat(7, 1fr);
    gap: 2px;
    padding: 4px;
  }

  .weekday {
    color: var(--color-faint);
    font-size: 10.5px;
    text-align: center;
  }

  .day {
    aspect-ratio: 1;
    border: 0;
    border-radius: var(--radius-control-compact);
    background: transparent;
    color: var(--color-text);
    font: inherit;
    font-size: 12px;
    font-variant-numeric: tabular-nums;
  }

  .day:hover {
    background: var(--row-hover);
  }

  .day.outside {
    color: var(--color-faint);
  }

  .day.today {
    box-shadow: inset 0 0 0 1px var(--color-border-strong);
    font-weight: 600;
  }

  .day.has {
    background: var(--color-fill-active);
  }
</style>
