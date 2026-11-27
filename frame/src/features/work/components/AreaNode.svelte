<script lang="ts">
  import { isComposing } from "$shared/lib/keyboard";
  import { NodeResizer, NodeToolbar, Position, type NodeProps, type Node } from "@xyflow/svelte";
  import { getContext, tick } from "svelte";
  import { on } from "svelte/events";
  import Icon from "$shared/ui/Icon";
  import ArrowShrink02Icon from "@hugeicons/core-free-icons/ArrowShrink02Icon";
  import PencilEdit01Icon from "@hugeicons/core-free-icons/PencilEdit01Icon";
  import { MinusSignIcon } from "../lib/icons";
  import { canvasAreaActions, canvasResize } from "../lib/canvas-context";
  import type { AreaData } from "../lib/canvas-model";
  import * as m from "$shared/i18n/messages";
  const resize = getContext<(active: boolean) => void>(canvasResize);
  const actions = getContext<
    | {
        fit: (id: string) => void;
        rename: (id: string, title: string) => void;
        remove: (id: string) => void;
      }
    | undefined
  >(canvasAreaActions);
  let { id, data, selected }: NodeProps<Node<AreaData, "area">> = $props();
  let editing = $state(false);
  let draft = $state("");
  let input = $state<HTMLInputElement>();
  async function edit() {
    draft = data.title;
    editing = true;
    await tick();
    input?.focus();
    input?.select();
  }
  function commit() {
    if (!editing) return;
    editing = false;
    const title = draft.trim();
    if (title && title !== data.title) actions?.rename(id, title);
  }
</script>

<NodeResizer
  onResizeStart={() => resize(true)}
  onResizeEnd={() => resize(false)}
  isVisible={selected}
  minWidth={240}
  minHeight={160}
  maxWidth={8192}
  maxHeight={8192}
  lineClass="work-resize-line"
  handleClass="work-resize-handle"
/>
{#if actions}<NodeToolbar position={Position.Top} offset={10}>
    <div class="bar" role="toolbar" aria-label={data.title}>
      <button type="button" onclick={() => actions.fit(id)}>
        <Icon icon={ArrowShrink02Icon} size={14} />{m.work_area_fit()}
      </button>
      <button type="button" onclick={edit}>
        <Icon icon={PencilEdit01Icon} size={14} />{m.work_area_rename()}
      </button>
      <span class="separator"></span>
      <button type="button" class="danger" onclick={() => actions.remove(id)}>
        <Icon icon={MinusSignIcon} size={14} />{m.work_area_remove()}
      </button>
    </div>
  </NodeToolbar>{/if}
<section class="area" class:selected aria-label={data.title}>
  <header class="area-title" {@attach (element) => on(element, "dblclick", edit)}>
    {#if editing}<input
        bind:this={input}
        bind:value={draft}
        class="name-input nodrag nopan"
        aria-label={m.work_area_title()}
        maxlength="128"
        onkeydown={(event) => {
          if (isComposing(event)) return;
          if (event.key === "Enter") {
            event.preventDefault();
            commit();
          } else if (event.key === "Escape") {
            event.preventDefault();
            event.stopPropagation();
            editing = false;
          }
        }}
        onblur={commit}
      />{:else}<span class="name">{data.title}</span>{/if}
    {#if data.count}<span class="count">{data.count}</span>{/if}
  </header>
</section>

<style>
  .area {
    box-sizing: border-box;
    block-size: 100%;
    border-radius: var(--radius-card);
    background: color-mix(in srgb, var(--color-fill) 60%, transparent);
    box-shadow: inset 0 0 0 1px var(--color-border);
    transition: box-shadow var(--motion-fast) var(--ease-smooth);
  }

  .area.selected {
    box-shadow:
      inset 0 0 0 1px var(--color-border-strong),
      0 0 0 2px var(--color-accent-soft);
  }

  .area-title {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 16px;
    color: var(--color-muted);
    cursor: grab;
  }

  .area-title:active {
    cursor: grabbing;
  }

  .name {
    font-size: var(--text-label);
    font-weight: 600;
    letter-spacing: 0.02em;
    text-transform: uppercase;
  }

  .name-input {
    min-inline-size: 0;
    inline-size: 220px;
    padding: 2px 6px;
    margin: -3px -7px;
    border: 1px solid var(--color-accent);
    border-radius: var(--radius-control-compact);
    background: var(--color-surface);
    color: var(--color-text);
    font: inherit;
    font-size: var(--text-label);
    font-weight: 600;
    letter-spacing: 0.02em;
    text-transform: uppercase;
    outline: none;
  }

  .count {
    padding: 0 6px;
    border-radius: var(--radius-capsule);
    background: var(--color-fill);
    font-size: var(--text-caption);
    font-variant-numeric: tabular-nums;
  }

  .bar {
    display: flex;
    align-items: center;
    gap: 2px;
    padding: 4px;
    border-radius: var(--radius-control);
    background: var(--color-float);
    box-shadow: var(--shadow-menu);
  }

  .bar button {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    block-size: 28px;
    padding: 0 10px;
    border: 0;
    border-radius: var(--radius-control-compact);
    background: transparent;
    color: var(--color-text);
    font: inherit;
    font-size: var(--text-label);
    font-weight: 500;
    cursor: default;
    transition: background-color var(--motion-instant) ease;
  }

  .bar button:hover {
    background: var(--color-control-hover);
  }

  .bar button.danger:hover {
    color: var(--color-danger);
  }

  .separator {
    inline-size: 1px;
    block-size: 18px;
    margin-inline: 4px;
    background: var(--color-border);
  }
</style>
