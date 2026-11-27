<script lang="ts" module>
  /** Where the pointer last moved. A list that scrolls or reflows under a
   *  resting pointer is sent a move from the same spot, and that is the list
   *  moving, not the user choosing a row. */
  let pointerAt: readonly [number, number] | null = null;
</script>

<script lang="ts">
  import {
    Search01Icon,
    Globe02Icon,
    CommandLineIcon,
    Clock01Icon,
    Note01Icon,
    StarIcon,
  } from "@hugeicons/core-free-icons";
  import type { IconSvgElement } from "@hugeicons/svelte";
  import * as m from "$shared/i18n/messages";
  import type { SearchResult } from "$shared/ipc/bindings";
  import Icon from "$shared/ui/Icon";
  import FavIcon from "$shared/ui/FavIcon";
  import { favicons } from "$domain/favicons";
  import { IS_MAC } from "$shared/platform";
  import { acceleratorKeys } from "$shared/lib/accelerator";
  import { matchRange, resultDetail } from "../lib/search-model";
  import type { Calculation } from "../lib/calculator";

  const line = (d: string, key: string) =>
    [
      "path",
      {
        d,
        stroke: "currentColor",
        strokeLinecap: "round",
        strokeLinejoin: "round",
        strokeWidth: "1.5",
        key,
      },
    ] as const;
  // The package's Calculator glyph, held here: every icon imported from the
  // package lands in one chunk that the note editor's graph also counts.
  const CALCULATOR = [
    line("M5.5 3V8M8 5.5L3 5.5", "0"),
    line("M8 16L6 18M6 18L4 20M6 18L8 20M6 18L4 16", "1"),
    line("M20 6L16 6", "2"),
    line("M20 18.5L16 18.5M20 15.5L16 15.5", "3"),
    line("M22 12L2 12", "4"),
    line("M12 22L12 2", "5"),
  ] as unknown as IconSvgElement;

  let {
    result = null,
    title,
    icon = null,
    keys = [],
    calculation,
    id,
    index,
    setsize,
    selected,
    query,
    onhover,
    onrun,
    variant = "compact",
  }: {
    result?: SearchResult | null;
    title: string;
    /** Set for a tool destination, which has no native result behind it. */
    icon?: IconSvgElement | null;
    /** A shortcut that runs this row, shown in place of its kind. */
    keys?: string[];
    calculation?: Calculation;
    id: string;
    index: number;
    setsize: number;
    selected: boolean;
    query: string;
    onhover: () => void;
    onrun: (event: MouseEvent) => void;
    /** The launcher is a place to read a list, not a dropdown under a field:
     *  its rows are taller, name what kind of thing each one is at the far
     *  edge, and leave selection to the list's own moving highlight. */
    variant?: "compact" | "launcher";
  } = $props();

  function pointermove(event: PointerEvent) {
    if (pointerAt?.[0] === event.screenX && pointerAt[1] === event.screenY) return;
    pointerAt = [event.screenX, event.screenY];
    onhover();
  }

  let launcher = $derived(variant === "launcher");
  let detail = $derived(result ? resultDetail(result) : "");
  let range = $derived(matchRange(title, query));
  // A site the user has reached is shown by its own mark. The clock belongs to
  // a recorded search, not to every visited page.
  let sited = $derived(
    !!result && (result.kind === "tab" || result.kind === "history" || result.kind === "url"),
  );
  let site = $derived(
    sited && result
      ? favicons.mark(result.icon, result.action.type === "OpenUrl" ? result.action.url : null)
      : null,
  );
  let image = $derived(site?.image ?? null);
  let glyph = $derived.by(() => {
    if (icon) return icon;
    if (calculation) return CALCULATOR;
    switch (result?.kind) {
      case "search":
      case "suggestion":
        return Search01Icon;
      case "search_history":
        return Clock01Icon;
      case "bookmark":
        return StarIcon;
      case "note":
        return Note01Icon;
      case "command":
        return CommandLineIcon;
      default:
        return Globe02Icon;
    }
  });
  let shortcut = $derived(
    keys.length ? keys : result?.kind === "command" ? acceleratorKeys(result.detail, IS_MAC) : [],
  );
  // The search row already names its engine inline; everything else says what
  // it is, so a mixed list needs no section headings.
  let kind = $derived.by(() => {
    if (calculation) return m.launcher_kind_calculator();
    switch (result?.kind) {
      case "tab":
        return m.launcher_kind_tab();
      case "history":
        return m.launcher_kind_history();
      case "search_history":
        return m.launcher_kind_recent();
      case "bookmark":
        return m.launcher_kind_bookmark();
      case "suggestion":
        return m.launcher_kind_suggestion();
      case "note":
        return m.launcher_kind_note();
      case "url":
        return m.launcher_kind_address();
      case "command":
        return m.launcher_kind_command();
      default:
        return "";
    }
  });
</script>

<button
  type="button"
  {id}
  class="row"
  class:launcher
  class:calculation={!!calculation}
  class:selected
  tabindex="-1"
  role="option"
  aria-selected={selected}
  aria-posinset={index + 1}
  aria-setsize={setsize}
  aria-keyshortcuts={shortcut.length ? shortcut.join("+") : undefined}
  onpointermove={pointermove}
  onpointerdown={(event) => {
    // Keep the field focused so the list does not dismiss itself out from
    // under the pointer before the click lands.
    if (event.pointerType !== "touch") event.preventDefault();
  }}
  onclick={onrun}
>
  <span class="glyph">
    {#if image}<FavIcon {image} tone={site?.tone} size={16} lit fallback={Globe02Icon} />
    {:else}<Icon icon={glyph} size={launcher ? 18 : 16} />{/if}
  </span>
  {#if calculation}<span class="text sum"
      ><span class="expression">{calculation.expression}</span><span
        class="equals"
        aria-hidden="true">=</span
      ><span class="answer">{calculation.text}</span></span
    >{:else}<span class="text"
      ><span class="title"
        >{#if range}{title.slice(0, range[0])}<mark>{title.slice(range[0], range[1])}</mark
          >{title.slice(range[1])}{:else}{title}{/if}</span
      >{#if detail}<span class="detail">{detail}</span>{/if}</span
    >{/if}
  {#if shortcut.length}<span class="keys" aria-hidden="true"
      >{#each shortcut as key, i (i)}<kbd>{key}</kbd>{/each}</span
    >{:else if launcher && kind}<span class="kind">{kind}</span>{/if}
</button>

<style>
  .row {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    height: 36px;
    padding: 0 10px;
    border: 0;
    border-radius: var(--radius-row);
    background: transparent;
    color: var(--color-text);
    text-align: start;
    cursor: default;
    transition: background-color var(--motion-instant) var(--ease-smooth);
  }

  /* Deliberately no per-row entry animation. The typed row's action carries the
     query, so its identity changes on every keystroke and it would re-fade as
     fast as the user types — which reads as exactly the lag it was meant to
     soften. The list settles by resizing instead. */

  /* One filled shape, no ring. An inset hairline here reads as a text field
     nested inside the list rather than as a selected row.

     Light chrome normally elevates by getting brighter, but that idiom assumes
     a grey ground: a white pill on a white result surface is invisible. Over
     this surface the selection is a light-touch overlay instead. */
  .row.selected {
    background: var(--color-fill-active);
  }

  :global(:root[data-theme="light"]) .row.selected {
    background: var(--color-fill-pressed);
  }

  .glyph {
    display: grid;
    place-items: center;
    flex: none;
    width: 18px;
    height: 18px;
    color: var(--color-muted);
  }

  .row.selected .glyph {
    color: var(--color-text);
  }

  .text {
    display: flex;
    flex: 1;
    align-items: baseline;
    gap: 10px;
    min-width: 0;
  }

  .title {
    flex: 0 1 auto;
    min-width: 0;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
    font-size: 13.5px;
    font-weight: 400;
    line-height: 20px;
    color: var(--color-muted);
  }

  .row.selected .title {
    color: var(--color-text);
  }

  /* The part the typing accounts for carries the weight; the rest recedes. */
  mark {
    background: none;
    color: var(--color-text);
    font-weight: 550;
  }

  .detail {
    flex: none;
    min-width: 0;
    max-width: 38%;
    margin-inline-start: auto;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
    text-align: end;
    font-size: 11.5px;
    color: var(--color-faint);
  }

  .keys {
    display: inline-flex;
    flex: none;
    gap: 3px;
  }

  kbd {
    display: inline-grid;
    place-items: center;
    box-sizing: border-box;
    min-width: 18px;
    height: 18px;
    padding: 0 4px;
    border-radius: 5px;
    background: var(--color-fill);
    color: var(--color-faint);
    font-family: var(--font-sans);
    font-size: 11px;
    font-weight: 500;
  }

  /* Launcher rows. The selection is drawn once by the list and glides between
     rows, so a row itself never fills; positioned so it paints above that
     highlight. The detail follows the title rather than being pushed to the
     far edge, which belongs to the row's kind. */
  .row.launcher {
    position: relative;
    gap: 12px;
    height: 44px;
    padding: 0 16px 0 15px;
    border-radius: var(--row-radius, var(--radius-card));
    transition: none;
  }

  .row.launcher.selected {
    background: transparent;
  }

  .row.launcher .glyph {
    width: 20px;
    height: 20px;
  }

  .row.launcher .title {
    color: var(--color-text);
    font-size: 14px;
    font-weight: 450;
    letter-spacing: -0.005em;
  }

  .row.launcher .detail {
    flex: 0 100 auto;
    max-width: none;
    margin-inline-start: 0;
    text-align: start;
    font-size: 13px;
  }

  /* An answer reads as a result, not a link: the sum steps back and what it
     comes to is set larger, with room for its figures. */
  .row.launcher.calculation {
    height: 56px;
  }

  .sum {
    align-items: baseline;
    gap: 12px;
  }

  .expression {
    min-width: 0;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
    font-size: 14px;
    color: var(--color-muted);
    font-variant-numeric: tabular-nums;
  }

  .equals {
    flex: none;
    font-size: 16px;
    color: var(--color-faint);
  }

  .answer {
    flex: none;
    font-size: 21px;
    font-weight: 500;
    letter-spacing: -0.012em;
    color: var(--color-text);
    font-variant-numeric: tabular-nums;
    user-select: text;
  }

  .kind {
    flex: none;
    font-size: 12.5px;
    color: var(--color-faint);
  }

  @media (prefers-reduced-motion: reduce) {
    .row {
      transition: none;
    }
  }

  @media (forced-colors: active) {
    .row.selected {
      outline: 1px solid Highlight;
    }
  }
</style>
