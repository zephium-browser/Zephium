<script lang="ts">
  import type { Snippet } from "svelte";
  import { onMount } from "svelte";
  import type { ToolHostProps } from "$session/tool-drafts.svelte";
  import { tools } from "../components/tool-views";
  import Icon from "$shared/ui/Icon";
  import IconButton from "$shared/ui/IconButton";
  import SegmentedControl from "$shared/ui/SegmentedControl";
  import {
    Add01Icon,
    ArrowExpand01Icon,
    ArrowLeft01Icon,
    Cancel01Icon,
    Search01Icon,
  } from "@hugeicons/core-free-icons";
  import * as m from "$shared/i18n/messages";
  import "./tools.css";
  let {
    tool,
    state,
    edit,
    onclose,
    searchLabel,
    searchOpen = true,
    searchFocus = true,
    onsearchdismiss,
    heading,
    filters,
    actions,
    canCompose = false,
    marked = false,
    onexpand,
    caption = true,
    scrolls = true,
    children,
    footer,
  }: ToolHostProps & {
    searchLabel?: string;
    /** False keeps the search field out of the frame until it is asked for, so
     *  a tool that rarely needs it does not spend a band on it. */
    searchOpen?: boolean;
    /** False where the field is always shown, so opening the tool does not pull
     *  focus away from wherever the reader already is. */
    searchFocus?: boolean;
    onsearchdismiss?: () => void;
    /** Replaces the tool's name, for a tool whose title is also a control. */
    heading?: Snippet;
    filters?: { value: string; label: string }[];
    /** Tool-specific header controls, placed before the shared ones. */
    actions?: Snippet;
    canCompose?: boolean;
    /** A glyph before the title. Titles stand alone; the dock already shows
     *  which tool this is. */
    marked?: boolean;
    /** Opens the tool as a page of its own, where it has one. */
    onexpand?: () => void;
    /** A finished tool does not describe itself as a layout study. */
    caption?: boolean;
    /** Set false when the view owns a scroller of its own, so the frame does
     *  not nest one inside another and swallow the child's height. */
    scrolls?: boolean;
    children?: Snippet;
    footer?: Snippet;
  } = $props();
  let meta = $derived(tools[tool]);
  let content: HTMLElement;
  // The field only exists while it is open, so mounting it is opening it, and
  // asking for it is asking to type in it. (`$state` is unavailable here: this
  // component already has a prop called `state`.)
  const takeFocus = (node: HTMLInputElement) => {
    if (searchFocus) node.focus();
  };
  onMount(() => {
    content.scrollTop = state.scrollTop;
  });
</script>

<section class="shared-tool" data-tool={tool} aria-label={meta.title()}>
  <header role="group" aria-label={meta.title()} class="shared-tool-header">
    <!-- The way back to the tabs leads the row, where reading starts. -->
    <IconButton icon={ArrowLeft01Icon} label={m.tool_back_to_tabs()} onclick={onclose} />
    {#if marked}<span class="shared-tool-icon"><Icon icon={meta.icon} size={16} /></span>{/if}
    {#if heading}{@render heading()}{:else}<h2>{meta.title()}</h2>{/if}
    {#if actions}{@render actions()}{/if}{#if canCompose}<IconButton
        icon={Add01Icon}
        label={tool === "notes" ? m.tool_add_note() : m.tool_add_task()}
        onclick={() => edit({ composing: !state.composing })}
      />{/if}{#if onexpand}<IconButton
        icon={ArrowExpand01Icon}
        label={m.tool_open_full()}
        onclick={onexpand}
      />{/if}
  </header>
  {#if (searchLabel && searchOpen) || filters}<div
      class="shared-tool-controls"
      data-tauri-drag-region="false"
    >
      {#if searchLabel && searchOpen}<div class="shared-tool-search">
          <Icon icon={Search01Icon} size={14} /><input
            use:takeFocus
            type="search"
            aria-label={searchLabel}
            placeholder={searchLabel}
            value={state.query}
            maxlength={1024}
            oninput={(event) => edit({ query: event.currentTarget.value })}
            onkeydown={(event) => {
              // An empty field lets Escape through to the host.
              if (event.key !== "Escape" || !onsearchdismiss || !event.currentTarget.value) return;
              event.preventDefault();
              event.stopPropagation();
              onsearchdismiss();
            }}
          />{#if state.query}<button
              type="button"
              class="shared-tool-clear"
              aria-label={m.tool_search_clear()}
              onclick={(event) => {
                edit({ query: "" });
                event.currentTarget.parentElement?.querySelector("input")?.focus();
              }}><Icon icon={Cancel01Icon} size={10} strokeWidth={2.4} /></button
            >{/if}
        </div>{/if}{#if filters}<SegmentedControl
          label={meta.title()}
          full
          value={state.filter}
          options={filters}
          onchange={(filter) => edit({ filter })}
        />{/if}
    </div>{/if}
  <div
    class="shared-tool-content"
    class:shared-tool-content-hosted={!scrolls}
    data-tauri-drag-region="false"
    bind:this={content}
    onscroll={() => edit({ scrollTop: content.scrollTop })}
  >
    {#if children}{@render children()}{:else}<div class="shared-tool-empty">
        <span><Icon icon={meta.icon} size={28} /></span>
        <h3>{meta.empty()}</h3>
        <p>{meta.description()}</p>
      </div>{/if}
  </div>
  {#if footer}{@render footer()}{/if}
  {#if caption}<footer class="shared-tool-caption">{m.tool_preview()}</footer>{/if}
</section>
