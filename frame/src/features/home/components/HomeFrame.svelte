<script lang="ts">
  import type { Component, ComponentProps, Snippet } from "svelte";
  import type HomeCanvas from "./HomeCanvas.svelte";
  import type { HomeIntent, HomeModel, Place } from "../lib/model";

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
  let Canvas = $state<Canvas | null>(null);
  // The canvas is asked for once the field has been painted, and only once there is something to draw.
  $effect(() => {
    if (!searched || Canvas) return;
    let live = true;
    requestAnimationFrame(() =>
      requestAnimationFrame(() => {
        if (!live) return;
        void load().then(
          (module) => {
            if (live) Canvas = module.default;
          },
          () => {},
        );
      }),
    );
    return () => {
      live = false;
    };
  });
</script>

<section class="home" style:--home-notch={`${notch}px`} style:--home-foot={`${foot}px`}>
  {#if ground}<div class="ground">{@render ground()}</div>{/if}
  {#if searched}
    <div class="canvas">
      {#if Canvas}<Canvas
          {home}
          {onintent}
          {picture}
          {mark}
          inset={{ top: notch, bottom: foot }}
          {onready}
        />{/if}
    </div>
  {:else if welcome}
    <div class="welcome">{@render welcome()}</div>
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
