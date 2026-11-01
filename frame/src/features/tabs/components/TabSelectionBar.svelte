<script lang="ts">
  import {
    Cancel01Icon,
    Delete02Icon,
    StarIcon,
    VolumeHighIcon,
    VolumeOffIcon,
    FolderTransferIcon,
  } from "@hugeicons/core-free-icons";
  import { tabs } from "$domain/tabs";
  import { settle, type Settlement } from "$domain/operations";
  import { commands } from "$shared/ipc/bindings";
  import * as m from "$shared/i18n/messages";
  import Icon from "$shared/ui/Icon";
  import * as notice from "$session/notice.svelte";
  import type { TabSelection } from "../lib/tab-selection.svelte";

  let { selection }: { selection: TabSelection } = $props();
  let busy = $state(false);
  const selected = $derived(tabs.tabs().filter((tab) => selection.value.ids.includes(tab.id)));
  const essentialIds = $derived(
    new Set(
      tabs
        .sidebarNodes()
        .flatMap((node) =>
          node.section === "favorites" && node.kind.type === "tab" ? [node.kind.tab_id] : [],
        ),
    ),
  );
  const removeEssential = $derived(selected.every((tab) => essentialIds.has(tab.id)));
  const keepable = $derived(selected.filter((tab) => tab.url && (tab.content ?? "web") === "web"));
  const audio = $derived(selected.filter((tab) => tab.audio?.can_mute));
  const unmute = $derived(audio.length > 0 && audio.every((tab) => tab.audio?.muted));
  const spaces = $derived(tabs.spaces().filter((space) => space.id !== tabs.activeSpaceId()));

  async function run(ids: readonly string[], action: (id: string) => Promise<Settlement>) {
    if (busy) return;
    busy = true;
    try {
      // Snapshot the targets before the first projection from native can prune them.
      const results = await Promise.allSettled(ids.map(async (id) => action(id)));
      if (
        results.some(
          (result) =>
            result.status === "rejected" ||
            result.value.outcome === "failed" ||
            result.value.outcome === "rejected",
        )
      ) {
        notice.show(m.tab_selection_failed(), "data");
      }
    } finally {
      busy = false;
    }
  }
</script>

{#if selected.length >= 2}
  <div
    class="selection-bar"
    role="group"
    aria-label={m.tab_selection_count({ count: selected.length })}
  >
    <div class="selection-heading">
      <span aria-live="polite">{m.tab_selection_count({ count: selected.length })}</span>
      <button
        type="button"
        class="selection-action"
        title={m.tab_selection_clear()}
        aria-label={m.tab_selection_clear()}
        onclick={() => selection.clear()}
      >
        <Icon icon={Cancel01Icon} size={14} />
      </button>
    </div>
    <fieldset class="selection-actions" disabled={busy}>
      <legend class="sr-only">{m.tab_selection_actions()}</legend>
      <button
        type="button"
        class="selection-action"
        title={m.tab_selection_close()}
        aria-label={m.tab_selection_close()}
        onclick={() =>
          void run(
            selected.map((tab) => tab.id),
            (id) => settle(commands.tabsClose(id)),
          )}
      >
        <Icon icon={Delete02Icon} size={16} />
      </button>
      <button
        type="button"
        class="selection-action"
        title={removeEssential ? m.native_remove_from_essentials() : m.native_add_to_essentials()}
        aria-label={removeEssential
          ? m.native_remove_from_essentials()
          : m.native_add_to_essentials()}
        disabled={!removeEssential && keepable.length === 0}
        onclick={() => {
          const essential = !removeEssential;
          const ids = (essential ? keepable : selected).map((tab) => tab.id);
          void run(ids, (id) => settle(commands.tabsSetEssential(id, essential, null)));
        }}
      >
        <Icon icon={StarIcon} size={16} />
      </button>
      <button
        type="button"
        class="selection-action"
        title={unmute ? m.tab_selection_unmute() : m.tab_selection_mute()}
        aria-label={unmute ? m.tab_selection_unmute() : m.tab_selection_mute()}
        disabled={audio.length === 0}
        onclick={() => {
          const muted = !unmute;
          const ids = audio.map((tab) => tab.id);
          void run(ids, (id) => tabs.setMuted(id, muted));
        }}
      >
        <Icon icon={unmute ? VolumeHighIcon : VolumeOffIcon} size={16} />
      </button>
      {#if tabs.spaces().length > 1 && spaces.length > 0}
        <label class="selection-move">
          <Icon icon={FolderTransferIcon} size={16} />
          <select
            class="selection-action"
            aria-label={m.native_move_to_space()}
            title={m.native_move_to_space()}
            value=""
            onchange={(event) => {
              const space = event.currentTarget.value;
              event.currentTarget.value = "";
              if (space)
                void run(
                  selected.map((tab) => tab.id),
                  (id) => tabs.moveTabToSpace(id, space),
                );
            }}
          >
            <option value="" disabled>{m.native_move_to_space()}</option>
            {#each spaces as space (space.id)}
              <option value={space.id}>{space.name}</option>
            {/each}
          </select>
        </label>
      {/if}
    </fieldset>
  </div>
{/if}

<style>
  .selection-bar {
    flex: none;
    margin: 4px var(--sidebar-inset);
    padding: 4px 8px;
    border-radius: var(--radius-row);
    background: var(--row-active);
    color: var(--color-text);
    font-size: var(--sidebar-row-text);
  }

  .selection-heading {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
  }

  .selection-actions {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    min-inline-size: 0;
    margin: 0;
    padding: 0;
    border: 0;
  }

  .selection-action {
    display: grid;
    place-items: center;
    inline-size: 28px;
    block-size: 28px;
    padding: 0;
    border: 0;
    border-radius: var(--radius-inset);
    background: transparent;
    color: var(--color-label-secondary);
    cursor: default;
  }

  .selection-action:disabled {
    opacity: 0.5;
  }

  .selection-action:hover:not(:disabled) {
    background: var(--row-hover);
    color: var(--color-text);
  }

  .selection-move {
    position: relative;
    display: grid;
    place-items: center;
    inline-size: 28px;
    block-size: 28px;
    border-radius: var(--radius-inset);
    color: var(--color-label-secondary);
  }

  .selection-move:hover {
    background: var(--row-hover);
  }

  .selection-move select.selection-action {
    position: absolute;
    inset: 0;
    opacity: 0;
  }

  .selection-move:focus-within {
    outline: 2px solid var(--color-accent);
    outline-offset: -2px;
  }

  .selection-move:has(select:disabled) {
    opacity: 0.5;
  }

  @media (forced-colors: active) {
    .selection-bar {
      border: 1px solid CanvasText;
    }
  }
</style>
