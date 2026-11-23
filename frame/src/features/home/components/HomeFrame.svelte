<script lang="ts">
  import { onMount, type Component, type ComponentProps, type Snippet } from "svelte";
  import { ArrowDown01Icon } from "@hugeicons/core-free-icons";
  import Icon from "$shared/ui/Icon";
  import * as m from "$shared/i18n/messages";
  import type HomeCanvas from "./HomeCanvas.svelte";
  import type { HomeIntent, HomeModel, Place } from "../lib/model";
  import { dayName } from "../lib/time";

  type Canvas = Component<ComponentProps<typeof HomeCanvas>>;
  let {
    home,
    onintent,
    picture,
    mark,
    field,
    welcome,
    figures,
    controls,
    ground,
    notch = 50,
    foot = 82,
    load = () => import("./HomeCanvas.svelte"),
    onready,
  }: {
    home: HomeModel;
    onintent: (intent: HomeIntent) => void;
    picture?: (key: string) => string | null;
    mark?: (place: Place) => ImageData | null;
    /** The field, in the band at the top. */
    field: Snippet;
    /** What shows before the first search of the day: the name and the time. */
    welcome?: Snippet;
    /** The day in figures, along the foot. */
    figures?: Snippet;
    /** The account and settings, in the top corner. */
    controls?: Snippet;
    /** What the page is drawn on, under everything. */
    ground?: Snippet;
    notch?: number;
    foot?: number;
    load?: () => Promise<{ default: Canvas }>;
    onready?: (enter: () => void) => void;
  } = $props();

  let searched = $derived((home.today.entries?.length ?? 0) > 0);
  let earlier = $derived(home.earlier.find((day) => day.count > 0) ?? null);
  // Before the day's first search, the days before it are reached past the welcome.
  let reached = $state(false);
  let shown = $derived(searched || reached);
  let Canvas = $state<Canvas | null>(null);
  let failed = $state(false);
  let attempt = $state(0);
  // The canvas is asked for once the field has been painted, and only once there is something to draw.
  $effect(() => {
    void attempt;
    if (!shown || Canvas || failed) return;
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

  // The way in from the field: onto the canvas once it is drawn, or past the welcome to the days before.
  let into: (() => void) | null = null;
  let waiting = false;
  function enter() {
    if (into) return into();
    if (!searched && !earlier) return;
    waiting = true;
    reach();
  }
  function ready(next: () => void) {
    into = next;
    if (!waiting) return;
    waiting = false;
    next();
  }
  onMount(() => onready?.(enter));
  function reach() {
    if (!searched && earlier) reached = true;
  }
  const typing = (target: EventTarget | null) =>
    target instanceof HTMLElement && !!target.closest("input, textarea, select, [contenteditable]");
  let frame = $state<HTMLElement>();
  // Scrolling down or ArrowDown past the welcome goes on to the days before.
  $effect(() => {
    const element = frame;
    if (!element || shown) return;
    const wheel = (event: WheelEvent) => {
      if (event.deltaY > 0) reach();
    };
    const keys = (event: KeyboardEvent) => {
      if (event.key !== "ArrowDown" || typing(event.target)) return;
      event.preventDefault();
      enter();
    };
    element.addEventListener("wheel", wheel, { passive: true });
    element.addEventListener("keydown", keys);
    return () => {
      element.removeEventListener("wheel", wheel);
      element.removeEventListener("keydown", keys);
    };
  });
</script>

<section
  class="home"
  bind:this={frame}
  style:--home-notch={`${notch}px`}
  style:--home-foot={`${foot}px`}
>
  {#if ground}<div class="ground">{@render ground()}</div>{/if}
  {#if shown}
    <div class="canvas">
      {#if Canvas}<Canvas
          {home}
          {onintent}
          {picture}
          {mark}
          inset={{ top: notch, bottom: foot }}
          onready={ready}
        />{/if}
    </div>
  {/if}
  {#if !searched && welcome}
    <div class="welcome" class:gone={!!Canvas}>{@render welcome()}</div>
    {#if earlier && !reached}
      <button type="button" class="earlier" onclick={enter}>
        <span>{dayName(earlier.key, home.today.key) ?? m.home_yesterday()}</span>
        <span class="count">{m.home_day_count({ count: earlier.count })}</span>
        <Icon icon={ArrowDown01Icon} size={13} strokeWidth={2} />
      </button>
    {/if}
  {/if}
  {#if failed}
    <p class="failed" role="status">
      <span>{m.home_canvas_failed()}</span>
      <button
        type="button"
        onclick={() => {
          failed = false;
          attempt++;
        }}>{m.home_try_again()}</button
      >
    </p>
  {/if}
  <div class="field">{@render field()}</div>
  {#if controls}<div class="controls">{@render controls()}</div>{/if}
  {#if figures}<div class="figures">{@render figures()}</div>{/if}
</section>

<style>
  .home {
    position: relative;
    block-size: 100%;
    overflow: hidden;
    background: var(--color-canvas);
    isolation: isolate;
  }

  .ground,
  .canvas {
    position: absolute;
    inset: 0;
  }

  /* Cards go out under the figures rather than meeting them. */
  .canvas {
    mask-image: linear-gradient(
      to bottom,
      black calc(100% - var(--home-foot) - 48px),
      transparent calc(100% - var(--home-foot) + 16px)
    );
  }

  .welcome {
    position: absolute;
    inset: var(--home-notch) 0 var(--home-foot);
    display: grid;
    place-items: center;
    pointer-events: none;
    transition:
      opacity var(--motion-base) var(--ease-smooth),
      visibility 0s linear var(--motion-base);
  }

  .welcome.gone {
    visibility: hidden;
    opacity: 0;
  }

  /* The days before, said once and quietly over the foot. */
  .earlier {
    position: absolute;
    z-index: 2;
    inset-block-end: calc(var(--home-foot) + 4px);
    inset-inline-start: 50%;
    display: flex;
    align-items: center;
    gap: 6px;
    block-size: 26px;
    padding-inline: 10px;
    border: 0;
    border-radius: var(--radius-capsule);
    background: none;
    color: var(--color-faint);
    font: inherit;
    font-size: var(--text-label);
    translate: -50% 0;
    cursor: default;
    transition: color var(--motion-instant) var(--ease-smooth);
  }

  .earlier:hover {
    color: var(--color-text);
  }

  .earlier .count {
    opacity: 0.8;
  }

  .failed {
    position: absolute;
    z-index: 1;
    inset-block-start: calc(var(--home-notch) + 24px);
    inset-inline: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    margin: 0;
    color: var(--color-muted);
    font-size: var(--text-label);
  }

  .failed button {
    block-size: 26px;
    padding-inline: 10px;
    border: 0;
    border-radius: var(--radius-control-compact);
    background: var(--color-fill);
    color: var(--color-text);
    font: inherit;
    cursor: default;
  }

  .failed button:hover {
    background: var(--color-fill-hover);
  }

  @media (prefers-reduced-motion: reduce) {
    .welcome {
      transition: none;
    }
  }

  :global(:root[data-reduce-motion="true"]) .welcome {
    transition: none;
  }

  .field {
    position: absolute;
    z-index: 2;
    inset-block-start: 0;
    inset-inline: 0;
    block-size: var(--home-notch);
    pointer-events: none;
  }

  .field > :global(*) {
    pointer-events: auto;
  }

  .controls {
    position: absolute;
    z-index: 2;
    inset-block-start: 9px;
    inset-inline-end: 12px;
  }

  .figures {
    position: absolute;
    z-index: 2;
    inset-inline: 0;
    inset-block-end: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    block-size: var(--home-foot);
    pointer-events: none;
  }

  .figures > :global(*) {
    pointer-events: auto;
  }
</style>
