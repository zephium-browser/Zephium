<script lang="ts">
  import { shortcutHint } from "../../lib/platform";
  import type { Editor } from "@tiptap/core";
  import { tick } from "svelte";
  import Icon from "$shared/ui/Icon";
  import {
    ArrowDown01Icon,
    CheckListIcon,
    Link01Icon,
    Tick02Icon,
  } from "@hugeicons/core-free-icons";
  import Unlink02Icon from "@hugeicons/core-free-icons/Unlink02Icon";
  import * as m from "$shared/i18n/messages";
  import { BLOCKS, activeBlock } from "../../lib/editor/blocks";
  import { toggleChecklist } from "../../lib/editor/behaviour";
  import { MARKS, markActive } from "../../lib/editor/marks";
  import { portal } from "../../lib/editor/portal";
  import { boundsOf, place, type Box } from "../../lib/editor/placement";
  import BlockMenu from "./BlockMenu.svelte";

  let {
    editor,
    revision,
    linking = $bindable(false),
  }: { editor: Editor; revision: number; linking?: boolean } = $props();

  const HEIGHT = 38;

  let bar = $state<HTMLDivElement>();
  let styleButton = $state<HTMLButtonElement>();
  let styles = $state(false);
  let href = $state("");
  let field = $state<HTMLInputElement>();
  let position = $state<{ left: number; top: number; side: "above" | "below" } | null>(null);
  /** Room for the bar across its notes area; a sidebar has less than a page. */
  let room = $state(Infinity);

  let active = $derived.by(() => {
    void revision;
    return {
      marks: Object.fromEntries(MARKS.map((mark) => [mark.id, markActive(editor, mark.id)])),
      link: editor.isActive("link"),
      checklist:
        editor.isActive("listItem", { checked: false }) ||
        editor.isActive("listItem", { checked: true }),
      block: BLOCKS.find((block) => block.id === activeBlock(editor))!,
    };
  });
  let compact = $derived(room < 360);

  /** Centred over the selection, or under it when there is no room above.
   *  Measured in the viewport, as the bar is fixed. */
  function reposition() {
    const { from, to } = editor.state.selection;
    const view = editor.view;
    const start = view.coordsAtPos(from);
    const end = view.coordsAtPos(to, -1);
    const text = view.dom.getBoundingClientRect();
    const oneLine = Math.abs(start.top - end.top) < 2;
    const selection: Box = {
      left: oneLine ? start.left : text.left,
      right: oneLine ? end.right : text.right,
      top: start.top,
      bottom: end.bottom,
    };
    const bounds = boundsOf(view.dom);
    room = bounds.right - bounds.left;
    const width = bar?.offsetWidth ?? 280;
    const placed = place(selection, { width, height: HEIGHT }, bounds, "above", {
      gap: 8,
      align: "center",
    });
    // A selection taller than the view leaves no side free; the bar then
    // rides the edge of the area rather than leave it.
    const top = Math.min(Math.max(placed.top, bounds.top), bounds.bottom - HEIGHT);
    position = { left: placed.left, top, side: placed.side };
  }

  $effect(() => {
    void revision;
    void tick().then(reposition);
  });

  $effect(() => {
    if (!bar) return;
    const again = () => reposition();
    // Its width changes with what it shows: a style's name, the link field.
    const resized = new ResizeObserver(again);
    resized.observe(bar);
    window.addEventListener("scroll", again, { capture: true, passive: true });
    window.addEventListener("resize", again);
    return () => {
      resized.disconnect();
      window.removeEventListener("scroll", again, { capture: true });
      window.removeEventListener("resize", again);
    };
  });

  $effect(() => {
    if (!linking) return;
    styles = false;
    href = String(editor.getAttributes("link").href ?? "");
    void tick().then(() => {
      field?.focus();
      field?.select();
    });
  });

  function applyLink() {
    const value = href.trim();
    linking = false;
    if (!value) {
      editor.chain().focus().extendMarkRange("link").unsetMark("link").run();
      return;
    }
    const url = /^[a-z][a-z0-9+.-]*:/iu.test(value) ? value : `https://${value}`;
    const mark = { href: url, title: null, form: "inline" };
    const chain = editor.chain().focus();
    // With nothing selected, the address is the link's text.
    if (editor.state.selection.empty && !editor.isActive("link"))
      chain.insertContent({ type: "text", text: value, marks: [{ type: "link", attrs: mark }] });
    else chain.extendMarkRange("link").setMark("link", mark);
    chain.run();
  }
</script>

<!-- Pressing a control keeps focus, and the selection, in the text. -->
<div
  bind:this={bar}
  use:portal
  class="note-format"
  class:below={position?.side === "below"}
  role="toolbar"
  aria-label={m.note_formatting()}
  tabindex="-1"
  style:left="{position?.left ?? -9999}px"
  style:top="{position?.top ?? -9999}px"
  style:--bar-room="{Number.isFinite(room) ? room : 400}px"
  onmousedown={(event) => {
    if (!(event.target as HTMLElement).closest("input")) event.preventDefault();
  }}
>
  {#if linking}
    <form
      class="format-link"
      onsubmit={(event) => {
        event.preventDefault();
        applyLink();
      }}
    >
      <Icon icon={Link01Icon} size={14} />
      <input
        bind:this={field}
        bind:value={href}
        type="url"
        inputmode="url"
        spellcheck="false"
        autocomplete="off"
        placeholder={m.note_link_placeholder()}
        aria-label={m.note_link()}
        onkeydown={(event) => {
          if (event.key === "Escape") {
            event.preventDefault();
            linking = false;
            editor.commands.focus();
          }
        }}
      />
      {#if active.link}<button
          type="button"
          class="format-button"
          aria-label={m.note_link_remove()}
          title={m.note_link_remove()}
          onclick={() => {
            href = "";
            applyLink();
          }}><Icon icon={Unlink02Icon} size={15} /></button
        >{/if}
      <button type="submit" class="format-button" aria-label={m.note_link()}
        ><Icon icon={Tick02Icon} size={15} /></button
      >
    </form>
  {:else}
    <button
      bind:this={styleButton}
      type="button"
      class="format-style"
      class:compact
      aria-haspopup="menu"
      aria-expanded={styles}
      aria-label={compact ? `${m.note_formatting()}: ${active.block.label()}` : undefined}
      title={compact ? active.block.label() : undefined}
      onclick={() => (styles = !styles)}
    >
      {#if compact}<Icon icon={active.block.icon} size={15} />{:else}<span
          >{active.block.label()}</span
        >{/if}<Icon icon={ArrowDown01Icon} size={12} /></button
    >
    <span class="format-rule" aria-hidden="true"></span>
    {#each MARKS as mark (mark.id)}
      <button
        type="button"
        class="format-button"
        aria-label={mark.label()}
        title="{mark.label()}  {mark.hint}"
        aria-pressed={active.marks[mark.id]}
        onclick={() => mark.toggle(editor)}><Icon icon={mark.icon} size={15} /></button
      >
    {/each}
    <span class="format-rule" aria-hidden="true"></span>
    <button
      type="button"
      class="format-button"
      aria-label={m.note_link()}
      title="{m.note_link()}  {shortcutHint('CmdOrCtrl+K')}"
      aria-pressed={active.link}
      onclick={() => (linking = true)}><Icon icon={Link01Icon} size={15} /></button
    >
    <button
      type="button"
      class="format-button"
      aria-label={m.note_checklist()}
      title="{m.note_checklist()}  {shortcutHint('CmdOrCtrl+Shift+L')}"
      aria-pressed={active.checklist}
      onclick={() => toggleChecklist(editor)}><Icon icon={CheckListIcon} size={15} /></button
    >
  {/if}
</div>
{#if styles && styleButton && !linking}<BlockMenu
    {editor}
    {revision}
    anchor={styleButton}
    prefer={position?.side === "below" ? "below" : "above"}
    onclose={() => (styles = false)}
  />{/if}

<style>
  /* Opaque, unlike a menu: it sits on the very text being formatted, and
     words showing through its controls would read as part of them. */
  .note-format {
    position: fixed;
    z-index: 60;
    display: flex;
    align-items: center;
    gap: 2px;
    box-sizing: border-box;
    height: 38px;
    padding: 4px;
    border-radius: var(--radius-control);
    background: var(--color-float);
    box-shadow: var(--shadow-float);
    transform-origin: 50% 100%;
    animation: format-in var(--motion-fast) var(--ease-out);
  }

  .note-format.below {
    transform-origin: 50% 0;
  }

  @keyframes format-in {
    from {
      opacity: 0;
      scale: 0.96;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .note-format {
      animation-name: format-fade;
    }

    @keyframes format-fade {
      from {
        opacity: 0;
      }
    }
  }

  .format-button,
  .format-style {
    display: grid;
    place-items: center;
    flex: none;
    width: 30px;
    height: 30px;
    padding: 0;
    border: 0;
    border-radius: var(--radius-inset);
    background: transparent;
    color: var(--color-label-secondary);
    cursor: default;
    transition:
      background-color var(--motion-instant) var(--ease-smooth),
      color var(--motion-instant) var(--ease-smooth);
  }

  .format-style {
    display: flex;
    gap: 4px;
    width: auto;
    padding-inline: 9px 6px;
    color: var(--color-text);
    font: inherit;
    font-size: var(--text-body);
    font-weight: 500;
  }

  .format-style.compact {
    padding-inline: 7px 5px;
  }

  .format-style > :global(svg:last-child) {
    color: var(--color-faint);
  }

  .format-button:hover,
  .format-style:hover,
  .format-style[aria-expanded="true"] {
    background: var(--color-fill-hover);
    color: var(--color-text);
  }

  .format-button[aria-pressed="true"] {
    background: var(--color-fill-active);
    color: var(--color-text);
  }

  .format-button:focus-visible,
  .format-style:focus-visible {
    outline: 2px solid var(--color-ring);
    outline-offset: -2px;
  }

  .format-rule {
    flex: none;
    width: 1px;
    height: 18px;
    margin-inline: 3px;
    background: var(--color-border-strong);
  }

  .format-link {
    display: flex;
    align-items: center;
    gap: 6px;
    width: min(340px, var(--bar-room) - 8px);
    padding-inline-start: 8px;
    color: var(--color-muted);
  }

  .format-link input {
    flex: 1;
    min-width: 0;
    height: 28px;
    padding: 0;
    border: 0;
    background: transparent;
    color: var(--color-text);
    font: inherit;
    font-size: var(--text-body);
    outline: none;
  }

  .format-link input::placeholder {
    color: var(--color-faint);
  }
</style>
