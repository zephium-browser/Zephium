<script lang="ts">
  import {
    Handle,
    Position,
    NodeResizer,
    NodeToolbar,
    useViewport,
    type NodeProps,
  } from "@xyflow/svelte";
  import { getContext, onMount, tick } from "svelte";
  import Icon from "$shared/ui/Icon";
  import type { WorkItemNode } from "../lib/canvas-model";
  import {
    canvasResize,
    canvasAction,
    canvasOpen,
    canvasFocusResult,
    canvasAreas,
    canvasArrival,
    canvasSeen,
  } from "../lib/canvas-context";
  import { arrive } from "$shared/canvas";
  import type { Duration } from "$shared/lib/motion";
  import { ArrowUpRight01Icon, MinusSignIcon, Target01Icon, Tick02Icon } from "../lib/icons";
  import TabCard from "./cards/TabCard.svelte";
  import NoteCard from "./cards/NoteCard.svelte";
  import MediaCard from "./cards/MediaCard.svelte";
  import ObjectiveCard from "./cards/ObjectiveCard.svelte";
  import ResultCard from "./cards/ResultCard.svelte";
  import SubjectCard from "./cards/SubjectCard.svelte";
  import CompareCard from "./cards/CompareCard.svelte";
  import FolderCard from "./cards/FolderCard.svelte";
  import PageCard from "./cards/PageCard.svelte";
  import SourcesCard from "./cards/SourcesCard.svelte";
  import PartRow from "./run/PartRow.svelte";
  import InputMark from "./run/InputMark.svelte";
  import ObjectHost from "./run/ObjectHost.svelte";
  import RequestText from "./cards/RequestText.svelte";
  import BlockHost from "./board/BlockHost.svelte";
  import BoardHead from "./board/BoardHead.svelte";
  import Trail from "./board/Trail.svelte";
  import * as m from "$shared/i18n/messages";
  const open = getContext<(id: string) => void>(canvasOpen);
  const action = getContext<(id: string, action?: string) => void>(canvasAction);
  const resize = getContext<(active: boolean) => void>(canvasResize);
  const focusResult = getContext<(id: string) => void>(canvasFocusResult);
  const areas = getContext<{ readonly list: readonly { id: string; title: string }[] }>(
    canvasAreas,
  );
  const arrival = getContext<((id: string) => Duration | null) | undefined>(canvasArrival);
  let { id, data, selected }: NodeProps<WorkItemNode> = $props();
  const seen = getContext<((id: string) => boolean) | undefined>(canvasSeen);
  /** Far from the view it is an empty box of its size; its content draws once it comes near. */
  const near = $derived(!seen || seen(id));
  let root = $state<HTMLDivElement>();
  // A card the run just placed rises into its group; one that was there already does not.
  onMount(() => {
    const motion = arrival?.(id);
    if (motion && root?.parentElement) arrive(root.parentElement, motion);
  });
  const type = $derived(data.type ?? (data.artifact ? "result" : "objective"));
  /** A card the run drew, not an element the person placed: it takes no orders. */
  const inert = $derived(
    ["page", "sources", "request", "block", "head", "trail", "part", "input"].includes(type),
  );
  /** A lane card takes its size from its lane. */
  const laid = $derived(!!data.size);
  /** What a block shows past its first rows opens in place, and closes again. */
  const OPENS = new Set(["table", "document", "code", "comparison", "gallery"]);
  const opens = $derived(type === "block" && OPENS.has(data.block?.data.kind ?? ""));
  /** A page held for a person opens the takeover, never a copy in Browse. */
  const waiting = $derived(data.page?.human?.phase === "waiting_for_human");
  /**
   * The toolbar stands on its object at the canvas's scale, never larger than
   * at 100% and never smaller than half, where it could no longer be read; a
   * part opens from its own name and needs none.
   */
  const viewport = useViewport();
  // Only a selected card follows the zoom: the rest never re-render as the view moves.
  const scale = $derived(selected ? Math.max(0.5, Math.min(1, viewport.current.zoom)) : 1);
  const tooled = $derived(
    type !== "head" &&
      type !== "trail" &&
      type !== "part" &&
      type !== "input" &&
      (type !== "block" || opens),
  );
</script>

<Handle
  type="target"
  position={Position.Left}
  isConnectable={false}
  tabindex={-1}
  aria-hidden="true"
/>
<NodeResizer
  onResizeStart={() => resize(true)}
  onResizeEnd={() => resize(false)}
  isVisible={selected && !laid}
  minWidth={160}
  minHeight={72}
  maxWidth={4096}
  maxHeight={4096}
  lineClass="work-resize-line"
  handleClass="work-resize-handle"
/>
<NodeToolbar isVisible={selected && tooled} position={Position.Top} offset={10 * scale}>
  <div class="bar" role="toolbar" aria-label={data.title} style:--scale={scale}>
    <button type="button" onclick={() => (waiting ? action(id, "help") : open(id))}>
      <Icon icon={ArrowUpRight01Icon} size={14} />{waiting
        ? m.work_human_open()
        : type === "block" && data.block?.open
          ? m.work_board_close()
          : m.work_env_open()}
    </button>
    {#if data.artifact && !inert}<button
        type="button"
        onclick={async () => {
          focusResult(id);
          await tick();
          root?.querySelector<HTMLElement>(".artifact-body")?.focus({ preventScroll: true });
        }}
      >
        <Icon icon={Target01Icon} size={14} />{m.work_env_focus_result()}
      </button>{/if}
    {#if !inert}<button type="button" onclick={() => action(id, "ask")}>{m.work_env_ask()}</button
      >{/if}
    {#if !inert && !data.actionLabel}<button
        type="button"
        class:on={!!data.decision}
        onclick={() => action(id, data.decision ? "unchoose" : "choose")}
        >{data.decision ? m.work_env_unchoose() : m.work_env_choose()}</button
      >{#if areas?.list.length}<select
          class="area nodrag nopan"
          aria-label={m.work_env_area_select()}
          value={data.area ?? ""}
          onchange={(event) => action(id, `area:${event.currentTarget.value}`)}
        >
          <option value="">{m.work_env_no_area()}</option>
          {#each areas.list as area (area.id)}<option value={area.id}>{area.title}</option>{/each}
        </select>{/if}{/if}
    {#if !inert && !data.actionLabel}<span class="separator"></span><button
        type="button"
        class="danger"
        onclick={() => action(id, "remove")}
      >
        <Icon icon={MinusSignIcon} size={14} />{m.work_env_remove()}
      </button>{/if}
  </div>
</NodeToolbar>
<div class="node-root" bind:this={root}>
  {#if !near}<span class="unseen" aria-hidden="true"></span>{:else}
    {#if data.decision}<span class="decision" title={data.decision}
        ><Icon icon={Tick02Icon} size={12} />{m.work_env_decided()}</span
      >{/if}
    {#if type === "block" && data.block}<BlockHost {id} item={data} {selected} />
    {:else if type === "head"}<BoardHead {id} item={data} />
    {:else if type === "trail"}<Trail item={data} {selected} />
    {:else if type === "tab" || type === "link"}<TabCard
        item={data}
        {selected}
        onplay={() => action(id, "play")}
      />
    {:else if type === "subject"}<SubjectCard item={data} {selected} />
    {:else if type === "folder"}<FolderCard item={data} {selected} />
    {:else if type === "page"}<PageCard
        item={data}
        {selected}
        onhelp={() => action(id, "help")}
        onwait={() => action(id, "wait")}
      />
    {:else if type === "note"}<NoteCard item={data} {selected} />
    {:else if type === "media"}<MediaCard item={data} {selected} />
    {:else if type === "objective" || type === "request"}<RequestText
        item={data}
        {selected}
        ontoggle={() => action(id, "expand-request")}
        onaction={() => action(id)}
      />
    {:else if type === "object" && data.object}<ObjectHost {id} item={data} {selected} />
    {:else if type === "input"}<InputMark item={data} />
    {:else if type === "sources"}<SourcesCard item={data} {selected} />
    {:else if type === "part"}<PartRow
        item={data}
        {selected}
        onopen={(page) => action(id, `page:${page}`)}
        onneed={(how) => action(id, `need:${how}`)}
        onwait={(page) => action(id, `wait:${page}`)}
        onlist={() => open(id)}
      />
    {:else if data.artifact?.content.kind === "matrix"}<CompareCard item={data} {selected} />
    {:else if type === "result" || data.artifact}<ResultCard
        {id}
        item={data}
        {selected}
        onaction={() => action(id)}
      />
    {:else}<ObjectiveCard item={data} {selected} onaction={() => action(id)} />{/if}
  {/if}
</div>
<!-- Where a line meets a node is its route's to say; the handles only let the flow draw it. -->
<Handle
  type="source"
  position={Position.Right}
  isConnectable={false}
  tabindex={-1}
  aria-hidden="true"
/>

<style>
  .node-root {
    display: contents;
  }

  .unseen {
    display: block;
    inline-size: 100%;
    block-size: 100%;
  }

  .decision {
    position: absolute;
    inset-block-start: -10px;
    inset-inline-start: 12px;
    z-index: 3;
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 2px 8px;
    border-radius: var(--radius-capsule);
    background: var(--color-success);
    color: var(--color-on-lit);
    font-size: 10.5px;
    font-weight: 600;
    letter-spacing: 0.02em;
    text-transform: uppercase;
    pointer-events: none;
  }

  .bar {
    transform: scale(var(--scale, 1));
    transform-origin: 50% 100%;
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

  .bar button.on {
    color: var(--color-success);
  }

  .bar .area {
    block-size: 28px;
    max-inline-size: 140px;
    padding: 0 8px;
    border: 0;
    border-radius: var(--radius-control-compact);
    background: transparent;
    color: var(--color-text);
    font: inherit;
    font-size: var(--text-label);
  }

  .bar .area:hover {
    background: var(--color-control-hover);
  }

  .separator {
    inline-size: 1px;
    block-size: 18px;
    margin-inline: 4px;
    background: var(--color-border);
  }

  :global(.work-resize-line) {
    border-color: transparent !important;
  }

  :global(.work-resize-handle) {
    inline-size: 8px !important;
    block-size: 8px !important;
    border-radius: 50% !important;
    border: 1px solid var(--color-border-strong) !important;
    background: var(--color-surface) !important;
  }
</style>
