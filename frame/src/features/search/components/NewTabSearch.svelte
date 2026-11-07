<script lang="ts">
  import { onMount, untrack } from "svelte";
  import { fade } from "svelte/transition";
  import { reducedMotion } from "$shared/lib/motion";
  import { Cancel01Icon } from "@hugeicons/core-free-icons";
  import * as m from "$shared/i18n/messages";
  import SearchField from "$shared/ui/SearchField";
  import IconButton from "$shared/ui/IconButton";
  import ResultList from "./ResultList.svelte";
  import { createSearchSurface } from "../lib/search-surface.svelte";
  import { parkCaret } from "$shared/lib/parked-caret";

  let {
    tabId,
    onactive,
  }: {
    tabId: string | null;
    /** Whether a query is in the field, so the page can make room for it. */
    onactive?: (active: boolean) => void;
  } = $props();

  // The search session is bound to one blank tab for the life of this
  // component; Shell keys the surface on the active tab, so a change is a
  // remount, never a reassignment underneath a live session.
  const surface = createSearchSurface({
    tabId: untrack(() => tabId),
    destinations: [],
    onTool: () => {},
  });

  let input = $state<HTMLInputElement>();
  let root: HTMLDivElement;
  let list = $state<HTMLElement>();
  let content = $state<HTMLElement>();
  let contentHeight = $state(0);
  /** Height is only animated once a first measurement exists, so the sheet
   *  does not grow out of nothing when it opens. */
  let measured = $state(false);
  // The sheet drops into whatever is below the field and never past it.
  let available = $state(420);

  let active = $derived(surface.query.trim().length > 0);
  let visible = $derived(
    surface.open &&
      active &&
      (surface.rows.length > 0 || surface.empty || surface.failed || surface.error !== "none"),
  );

  $effect(() => {
    surface.setList(list);
  });
  $effect(() => {
    const report = onactive;
    const value = active;
    untrack(() => report?.(value));
  });
  $effect(() => () => untrack(() => onactive)?.(false));
  $effect(() => {
    // Reading the completion is what makes this run on each settled answer.
    void surface.query;
    surface.applyCompletion(input);
  });
  $effect(() => {
    const element = content;
    if (!element) {
      measured = false;
      return;
    }
    // Sizing the sheet to its content turns a result arriving into a single
    // settling movement rather than a jump.
    const observer = new ResizeObserver(([entry]) => {
      // Border box, because the padding is part of what the sheet must be
      // tall enough to show; a content-box measurement leaves a scrollbar.
      contentHeight = entry!.borderBoxSize[0]!.blockSize;
      requestAnimationFrame(() => (measured = true));
    });
    observer.observe(element);
    return () => observer.disconnect();
  });

  function measure() {
    available = Math.max(
      160,
      Math.min(520, window.innerHeight - root.getBoundingClientRect().bottom - 32),
    );
  }

  function keydown(event: KeyboardEvent) {
    // See LauncherPanel: WebKit's committing keydown arrives after compositionend.
    if (surface.composing || event.isComposing || event.keyCode === 229) return;
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      void surface.move(event.key === "ArrowDown" ? 1 : -1);
    } else if (event.key === "Escape") {
      if (surface.dismiss()) event.preventDefault();
    } else if (event.key === "Enter") {
      event.preventDefault();
      surface.submit();
    }
  }

  onMount(() => {
    input?.focus();
    measure();
    window.addEventListener("resize", measure);
    const stop = surface.mount();
    const unpark = input ? parkCaret(input) : () => {};
    return () => {
      window.removeEventListener("resize", measure);
      unpark();
      stop();
    };
  });
</script>

<div
  bind:this={root}
  class="dock"
  data-active={active}
  data-open={visible}
  style:--available={`${available}px`}
  onfocusout={(event) => {
    if (event.relatedTarget instanceof Node && root.contains(event.relatedTarget)) return;
    surface.blurred();
  }}
>
  <div class="bar" data-glass-text>
    <SearchField
      size="page"
      label={m.search_web()}
      placeholder={m.search_web_short()}
      value={surface.query}
      bind:ref={input}
      oninput={surface.changed}
      onsubmit={() => surface.submit()}
      inputProps={{
        role: "combobox",
        "aria-controls": "newtab-search-results",
        "aria-expanded": visible,
        "aria-autocomplete": "both",
        "aria-activedescendant":
          visible && surface.selectedIndex >= 0
            ? `newtab-search-results-option-${surface.selectedIndex}`
            : undefined,
        maxlength: 2048,
        spellcheck: false,
        autocapitalize: "off",
        autocorrect: "off",
        // Room for the clear control, which is placed over the field rather
        // than threaded through the shared primitive for one caller's sake.
        style: surface.query ? "padding-inline-end: 40px" : undefined,
        onkeydown: keydown,
        onfocus: () => {
          measure();
          surface.focused();
        },
        oncompositionstart: surface.compositionStart,
        oncompositionend: (event) => surface.compositionEnd(event.currentTarget.value),
      }}
    />
    {#if surface.query}
      <span class="clear">
        <IconButton
          icon={Cancel01Icon}
          label={m.panel_clear_search()}
          onclick={() => {
            surface.changed("");
            input?.focus();
          }}
        />
      </span>
    {/if}
  </div>

  {#if visible}
    <div
      class="sheet"
      data-measured={measured}
      style:block-size={contentHeight ? `min(${contentHeight}px, var(--available))` : undefined}
      out:fade={{ duration: reducedMotion() ? 0 : 120 }}
    >
      <div class="scroll">
        <div class="content" bind:this={content}>
          {#if surface.failed || surface.error !== "none"}
            <div class="failure" role="alert">
              {surface.error === "too_long" ? m.panel_query_too_long() : m.panel_action_failed()}
              <button type="button" onclick={surface.retry}>{m.panel_retry()}</button>
            </div>
          {/if}
          {#if surface.empty}<p class="empty">{m.panel_no_results()}</p>{/if}
          <ResultList
            rows={surface.rows}
            selected={surface.selectedId}
            query={surface.answered}
            listId="newtab-search-results"
            busy={surface.pending}
            variant="launcher"
            bind:ref={list}
            onhover={surface.hover}
            onrun={surface.activate}
          />
        </div>
      </div>
    </div>
  {/if}
</div>

<style>
  /* The field fills the notch, centred in it with a little glass all round. */
  .dock {
    position: relative;
    display: flex;
    align-items: center;
    block-size: 100%;
    padding-block: 3px;
    text-align: start;
  }

  .bar {
    position: relative;
    inline-size: 100%;
  }

  /* The notch is the field's shape and material, so the field inside it is
     only its words. Set here rather than as a size of the shared field, which
     every page would pay for on behalf of this one. */
  .dock .bar :global(.ui-search[data-size]),
  .dock .bar :global(.ui-search[data-size]:hover),
  .dock .bar :global(.ui-search[data-size]:focus-within) {
    min-height: 44px;
    padding-inline: 18px;
    gap: 11px;
    border-radius: 0;
    background: transparent;
    box-shadow: none;
  }

  .dock .bar :global(.ui-search[data-size] svg) {
    inline-size: 16px;
    block-size: 16px;
  }

  .dock .bar :global(.ui-search[data-size] input) {
    height: 44px;
    font-size: 15px;
    letter-spacing: -0.008em;
  }

  .clear {
    position: absolute;
    inset-block: 0;
    inset-inline-end: 10px;
    display: flex;
    align-items: center;
  }

  /* The results drop out of the notch into the page below it, read as the
     launcher's list: rows that name their kind at the edge and one
     highlight that glides between them. */
  .sheet {
    --row-radius: calc(var(--radius-panel) - 6px);
    --gap: 6px;

    position: absolute;
    inset-block-start: calc(100% + var(--gap));
    inset-inline: 0;
    z-index: 1;
    max-block-size: var(--available);
    overflow: hidden;
    border-radius: var(--radius-panel);

    /* A sheet of frosted glass on the sky: the sky's own frost, laid where
       the picture lies beneath it, under the glass's tint. Stretching a
       small picture is the blur, so nothing is blurred as it opens. */
    background:
      linear-gradient(var(--sky-card), var(--sky-card)),
      var(--sky-frost, none) var(--sky-frost-x, 0) calc(var(--sky-frost-y, 0px) - var(--gap)) /
        var(--sky-frost-size, cover) no-repeat,
      var(--color-raised);
    box-shadow:
      inset 0 0 0 0.5px var(--sky-card-edge, var(--color-border)),
      inset 0 1px 0 var(--sky-card-rim, transparent),
      var(--shadow-float);
    transform-origin: 50% 0;
    animation: sheet-in 340ms var(--ease-emphasized) backwards;

    /* Its own layer while it shows: as it grows and settles, the sky under
       it is uncovered from what is already painted, never painted again. */
    will-change: transform;
  }

  /* The height goes straight for each answer as it lands, longer or
     shorter, in one long settle that picks up from wherever it is when the
     next answer comes mid-way. */
  .sheet[data-measured="true"] {
    transition: block-size var(--motion-slow) var(--ease-emphasized);
  }

  /* It unfolds out of the field: the edge travels down from the top while
     the sheet settles into place. The clip reaches past the sides and foot
     so its shadow is never cut, and only the opening is animated. */
  @keyframes sheet-in {
    from {
      opacity: 0;
      transform: translateY(-6px);
      clip-path: inset(0 -48px calc(100% - 12px) -48px);
    }

    to {
      clip-path: inset(0 -48px -48px -48px);
    }
  }

  .scroll {
    max-block-size: var(--available);
    overflow-y: auto;
    overscroll-behavior: contain;
  }

  /* Padding lives on the measured element so the animated height accounts for
     it. */
  .content {
    padding: 6px;
  }

  .empty {
    margin: 0;
    padding: 22px 12px;
    color: var(--color-faint);
    font-size: 13px;
    text-align: center;
  }

  .failure {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 8px 10px;
    color: var(--color-danger);
    font-size: 12px;
  }

  .failure button {
    padding: 5px 10px;
    border: 0;
    border-radius: var(--radius-inset);
    background: var(--color-fill);
    color: var(--color-text);
    cursor: default;
  }

  @media (prefers-reduced-motion: reduce) {
    .sheet {
      animation: none;
    }

    .sheet[data-measured="true"] {
      transition: none;
    }
  }

  :global(:root[data-reduce-motion="true"]) .sheet {
    animation: none;
  }

  :global(:root[data-reduce-motion="true"]) .sheet[data-measured="true"] {
    transition: none;
  }

  @media (forced-colors: active) {
    .sheet {
      border: 1px solid CanvasText;
    }
  }
</style>
