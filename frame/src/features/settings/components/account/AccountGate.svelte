<script lang="ts">
  import { untrack, type Snippet } from "svelte";
  import { decode, placeSky, SCENES, scene as skyScene, seen, type Scene } from "$shared/sky";

  let { children, foot }: { children: Snippet; foot?: Snippet } = $props();

  let width = $state(0);
  let height = $state(0);

  // Day over a light page, night over a dark one, as on the new tab.
  const scene = $derived(skyScene());
  // The horizon sits low enough that the sheet stands over open sky and the
  // clouds have dissolved into the page by its foot.
  const sky = $derived(placeSky(width, Math.round(height * 0.72)));

  let shown = $state<Scene | null>(seen(untrack(() => scene)) ? untrack(() => scene) : null);
  let failed = $state<Scene | null>(null);
  $effect(() => {
    const wanted = scene;
    let live = true;
    void decode(wanted).then(
      () => {
        if (live) shown = wanted;
      },
      () => {
        if (live) failed = wanted;
      },
    );
    return () => {
      live = false;
    };
  });
</script>

<!--
  Signing in is the Account page's whole scene: the new tab's sky, static,
  with the app's own icon over it and what the page asks of you in front.
  A sky that cannot be decoded leaves the page its own colour.
-->
<div
  class="account-gate"
  data-sky={scene}
  data-ready={shown === scene}
  data-failed={failed === scene}
  bind:clientWidth={width}
  bind:clientHeight={height}
>
  <div
    class="sky"
    aria-hidden="true"
    style:background-image={`url("${SCENES[scene]}")`}
    style:background-size={`${sky.w}px ${sky.h}px`}
    style:background-position={`${sky.x}px ${sky.y}px`}
  ></div>
  <div class="stage">
    <img
      class="mark"
      src="/zephium-icon@2x.png"
      width="64"
      height="64"
      alt="Zephium"
      draggable="false"
    />
    {@render children()}
  </div>
  {#if foot}<div class="foot">{@render foot()}</div>{/if}
</div>

<style>
  /* The scene is the whole pane: Settings' own padding and width limit step
     aside for it, and each level hands it the pane's height. */
  :global(.settings-content:has(.account-gate)),
  :global(.settings-page:has(.account-gate)),
  :global(.settings-view:has(.account-gate)) {
    display: grid;
  }

  :global(.settings-page:has(.account-gate)) {
    max-inline-size: none;
    margin: 0;
    padding: 0;
  }

  /* A sky that rose into place would leave the pane's top bare and push its
     foot past the edge while it did: the scene only fades. */
  :global(.settings-view:has(.account-gate)) {
    animation-name: stage-fade;
  }

  .account-gate {
    position: relative;
    display: grid;
    grid-template-rows: 1fr auto;
    min-inline-size: 0;
    overflow: hidden;
    background: var(--sky-paper);
    isolation: isolate;
  }

  .sky {
    position: absolute;
    inset: 0;
    background-repeat: no-repeat;
    opacity: 0;
    transition: opacity 700ms var(--ease-out);
  }

  .account-gate[data-ready="true"] .sky {
    opacity: 1;
  }

  /* Open sky above, the sheet beneath it, whatever room is left below: the
     optical middle of the pane sits well above its true one. */
  .stage {
    position: relative;
    display: flex;
    flex-direction: column;
    align-items: center;
    padding: clamp(32px, 6vh, 56px) clamp(16px, 4cqi, 32px);
  }

  .stage::before,
  .stage::after {
    flex: 1 1 0;
    min-block-size: 0;
    content: "";
  }

  .stage::before {
    flex-grow: 1;
  }

  .stage::after {
    flex-grow: 3;
  }

  /* What waits at the page's foot, on the page itself rather than the sky. */
  .foot {
    position: relative;
    display: flex;
    justify-content: center;
    padding: 0 clamp(16px, 4cqi, 32px) 20px;
  }

  /* The app's own icon, its squircle and shadow included, the one mark
     that reads the same over either sky. */
  .mark {
    flex: none;
    inline-size: 64px;
    block-size: 64px;
    margin-block-end: 24px;
    filter: drop-shadow(0 10px 24px var(--sky-shadow));
    opacity: 0;
    translate: 0 6px;
    transition:
      opacity 700ms var(--ease-out),
      translate 700ms var(--ease-emphasized);
  }

  .account-gate:is([data-ready="true"], [data-failed="true"]) .mark {
    opacity: 1;
    translate: 0 0;
  }

  @media (prefers-reduced-motion: reduce) {
    .mark {
      translate: none;
    }
  }

  @media (forced-colors: active) {
    .account-gate {
      background: Canvas;
    }

    .sky {
      display: none;
    }

    .mark {
      opacity: 1;
    }
  }
</style>
