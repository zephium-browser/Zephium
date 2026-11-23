<script lang="ts">
  import { onMount, untrack, type Component, type ComponentProps } from "svelte";
  import * as m from "$shared/i18n/messages";
  import type { MapTarget } from "$shared/ipc/bindings";
  import {
    home,
    suggest,
    type Entry,
    type HomeField,
    type HomeModel,
    type Place,
    type Refused,
    type Search,
  } from "$domain/home";
  import { favicons } from "$domain/favicons";
  import { tabs } from "$domain/tabs";
  import { latest as shownByNative, taken as shownTaken } from "$session/home-show.svelte";
  import type HomeCanvas from "./HomeCanvas.svelte";
  import { Front } from "../lib/front.svelte";
  import type { HomeIntent } from "../lib/intent";

  type Canvas = Component<ComponentProps<typeof HomeCanvas>>;
  let {
    top = 0,
    foot = 82,
    kept = true,
    load = () => import("./HomeCanvas.svelte"),
    onfield,
    onask,
    onsearching,
    onheld,
    onready,
  }: {
    /** How much of the top the field's band covers. */
    top?: number;
    /** How much of the foot the page's figures cover. */
    foot?: number;
    /** False in a private window, where nothing searched is remembered. */
    kept?: boolean;
    load?: () => Promise<{ default: Canvas }>;
    /** Gives the keyboard back to the field. */
    onfield?: () => void;
    /** Asks the same words again, as the field would. */
    onask?: (text: string) => void;
    /** Whether there are searches to draw, so the welcome can leave. */
    onsearching?: (searching: boolean) => void;
    /** What the field may offer from Home: the rows of the search in front first. */
    onheld?: (held: HomeField | null) => void;
    /** Hands over the way in from the field. */
    onready?: (enter: () => void) => void;
  } = $props();

  onMount(() => home.retain());

  let model: HomeModel = $derived(home.model());
  let entries = $derived<readonly Entry[]>(
    [model.today, ...model.earlier].flatMap((day) => day.entries),
  );
  let searched = $derived(entries.length > 0);
  // Said once native has answered, so the welcome does not flash back while Home is read again.
  $effect(() => {
    if (home.ready()) onsearching?.(searched);
  });

  const front = new Front();
  // A search that arrived while Home was drawn: its results page may stand in for it, once.
  let fresh = $state.raw<{ search: string; acted: boolean } | null>(null);
  const resulted: Record<string, true> = {};
  $effect.pre(() => {
    if (!home.ready()) return;
    const newest = entries.find((entry) => entry.kind !== "address")?.id ?? null;
    const held = new Set(entries.map((entry) => entry.id));
    untrack(() => {
      if (front.follow(newest, held) && newest) fresh = { search: newest, acted: false };
    });
  });
  // Anything the person does after a search arrives is theirs to have done.
  $effect(() => {
    if (!fresh || fresh.acted) return;
    const acted = () => {
      if (fresh) fresh = { ...fresh, acted: true };
    };
    document.addEventListener("pointerdown", acted, true);
    document.addEventListener("keydown", acted, true);
    return () => {
      document.removeEventListener("pointerdown", acted, true);
      document.removeEventListener("keydown", acted, true);
    };
  });
  const nothingDrawn = (search: Search) =>
    search.kind === "place"
      ? search.place.state === "failed"
      : search.kind === "compare" && search.places.every((place) => place.state === "failed");
  // A map that found nothing to draw leaves its results page to show, once, and only untouched.
  $effect(() => {
    const waiting = fresh;
    if (!waiting || waiting.acted || resulted[waiting.search]) return;
    if (front.lead !== waiting.search) return;
    const search = entries.find((entry) => entry.id === waiting.search);
    if (!search || search.kind === "address" || !nothingDrawn(search)) return;
    resulted[waiting.search] = true;
    void home.openResults(waiting.search);
  });

  // Native shows a search after a page of it closes or its group title is clicked.
  let marked = $state.raw<{ search: string; entry: string } | null>(null);
  $effect(() => {
    const shown = shownByNative();
    if (!shown || !home.ready()) return;
    if (!entries.some((entry) => entry.id === shown.search)) return;
    untrack(() => {
      front.show(shown.search);
      marked = shown.row ? { search: shown.search, entry: shown.row.entry } : null;
    });
    shownTaken(shown.serial);
  });

  let notice = $state<Refused | null>(null);
  const said = (refused: Refused | null) => {
    notice = refused;
  };
  const opening = (search: string, target: MapTarget, slot?: number) =>
    void home.open(search, target, slot).then(said);
  const find = (id: string) => entries.find((entry) => entry.id === id);
  function act(intent: HomeIntent) {
    notice = null;
    switch (intent.kind) {
      case "row":
        return opening(intent.search, { kind: "entry", id: intent.row });
      case "inside":
        return void home.openInside(intent.search, intent.row);
      case "fold":
        return home.foldInside(intent.search, intent.row);
      case "site":
        return opening(intent.search, { kind: "site" }, Number(intent.place));
      case "social":
        return opening(
          intent.search,
          { kind: "social", network: intent.network },
          Number(intent.place),
        );
      case "result":
        return opening(intent.search, { kind: "also", index: Number(intent.result) }, 0);
      case "results":
        return void home.openResults(intent.search).then(said);
      case "page": {
        const entry = find(intent.search);
        const tab =
          entry?.kind === "pages" ? entry.pages.find((one) => one.id === intent.page)?.tab : null;
        if (tab) tabs.activate(tab);
        return;
      }
      case "address": {
        const entry = find(intent.entry);
        if (entry?.kind === "address" && entry.tab) tabs.activate(entry.tab);
        return;
      }
      case "forget":
        return void home.forget(intent.search).then(said);
      case "again": {
        const entry = find(intent.search);
        if (entry && entry.kind !== "address") onask?.(entry.text);
        return;
      }
      case "field":
        return onfield?.();
    }
  }
  const mark = (place: Place) => favicons.image(place.icon);

  let Canvas = $state<Canvas | null>(null);
  let failed = $state(false);
  let attempt = $state(0);
  // The canvas is asked for once the field has been painted, and only once there is something to draw.
  $effect(() => {
    void attempt;
    if (!searched || Canvas || failed) return;
    let live = true;
    requestAnimationFrame(() =>
      requestAnimationFrame(() => {
        if (!live) return;
        void load().then(
          (module) => {
            if (live) Canvas = module.default;
          },
          () => {
            if (live) failed = true;
          },
        );
      }),
    );
    return () => {
      live = false;
    };
  });

  // The way in from the field goes onto the canvas once it is drawn.
  let into: (() => void) | null = null;
  let waiting = false;
  function enter() {
    if (into) return into();
    if (searched) waiting = true;
  }
  function ready(next: () => void) {
    into = next;
    if (!waiting) return;
    waiting = false;
    next();
  }
  onMount(() => onready?.(enter));
  onMount(() => {
    onheld?.({
      suggest: (query) => suggest(model, front.lead, query),
      run: (suggestion) => {
        notice = null;
        if (suggestion.kind === "row")
          opening(suggestion.search, { kind: "entry", id: suggestion.entry });
        else void home.focus(suggestion.search).then(said);
      },
    });
    return () => onheld?.(null);
  });

  const NOTICE: Record<Refused, () => string> = {
    gone: m.home_refused_gone,
    busy: m.home_refused_busy,
    unavailable: m.home_open_failed,
    failed: m.home_open_failed,
  };
</script>

<div class="home" style:--home-top={`${top}px`} style:--home-foot={`${foot}px`}>
  {#if searched}
    <div class="canvas">
      {#if Canvas}<Canvas
          home={model}
          open={front.open}
          lead={front.lead}
          centre={front.centre}
          {marked}
          onopen={(search) => {
            marked = null;
            front.choose(search);
          }}
          onintent={act}
          {mark}
          inset={{ top, bottom: foot }}
          onready={ready}
        />{/if}
    </div>
  {/if}
  {#if failed}
    <p class="status" role="status">
      <span>{m.home_canvas_failed()}</span>
      <button
        type="button"
        onclick={() => {
          failed = false;
          attempt++;
        }}>{m.home_try_again()}</button
      >
    </p>
  {:else if notice}
    <p class="status" role="status"><span>{NOTICE[notice]()}</span></p>
  {:else if !kept}
    <p class="status quiet">{m.home_private()}</p>
  {/if}
</div>

<style>
  .home {
    position: absolute;
    inset: 0;
    pointer-events: none;
  }

  .canvas,
  .status {
    pointer-events: auto;
  }

  /* Cards go out under the figures rather than meeting them. */
  .canvas {
    position: absolute;
    inset: 0;
    mask-image: linear-gradient(
      to bottom,
      black calc(100% - var(--home-foot) - 48px),
      transparent calc(100% - var(--home-foot) + 16px)
    );
  }

  .status {
    position: absolute;
    z-index: 1;
    inset-block-start: calc(var(--home-top) + 16px);
    inset-inline: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    inline-size: max-content;
    margin: 0 auto;
    color: var(--color-muted);
    font-size: var(--text-label);
  }

  .quiet {
    color: var(--color-faint);
    pointer-events: none;
  }

  .status button {
    block-size: 26px;
    padding-inline: 10px;
    border: 0;
    border-radius: var(--radius-control-compact);
    background: var(--color-fill);
    color: var(--color-text);
    font: inherit;
    cursor: default;
  }

  .status button:hover {
    background: var(--color-fill-hover);
  }
</style>
