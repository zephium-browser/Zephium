<script lang="ts">
  import { onMount, untrack, type Snippet } from "svelte";
  import { CheckListIcon, Note01Icon, PuzzleIcon, Shield01Icon } from "@hugeicons/core-free-icons";
  import * as m from "$shared/i18n/messages";
  import { browserImport } from "$domain/browser-import";
  import { tabs } from "$domain/tabs";
  import { preferences } from "$domain/preferences";
  import { commands, type WorkLocalServerV1 } from "$shared/ipc/bindings";
  import Button from "$shared/ui/Button";
  import Icon from "$shared/ui/Icon";
  import KeyHint from "$shared/ui/KeyHint";
  import { IS_MAC, IS_WINDOWS } from "$shared/platform";
  import { reducedMotion } from "$shared/lib/motion";
  import { WORDMARK } from "$shared/lib/wordmark";
  import * as motion from "$session/motion.svelte";
  import { SITES, SITE_URLS } from "../lib/catalog";
  import { scene } from "$shared/sky";
  import { cover, frame, land, NEWTAB, PAGE, SHOTS, STAGE, type Shot } from "../lib/camera";
  import { neighbour, position, stepsFor, type Step } from "../lib/flow";
  import { formName, type Box } from "../lib/formation";
  import { keptSites } from "../lib/kept";
  import type { Mind } from "../lib/minds";
  import Window from "./Window.svelte";
  import Import from "./steps/Import.svelte";
  import Essentials from "./steps/Essentials.svelte";
  import Launcher from "./steps/Launcher.svelte";
  import Models from "./steps/Models.svelte";

  let {
    onfinish,
    windowControls,
  }: {
    /** Hands the window to the browser, on its new tab; false if native
     *  refused. */
    onfinish: () => Promise<boolean>;
    windowControls?: Snippet;
  } = $props();

  let step = $state<Step>("welcome");
  /** Welcome's own beats: the dots gathering, the name standing, and the
   *  dots gone, with nothing of them left running. */
  let phase = $state<"waiting" | "forming" | "formed" | "still">("waiting");
  let leaving = $state(false);
  let notice = $state<string | null>(null);

  let width = $state(0);
  let height = $state(0);
  let fit = $derived(width && height ? Math.min(width / STAGE.width, height / STAGE.height) : 1);
  // The window's whole view, in stage pixels: the stage is centred in it.
  let view = $derived({ width: width / fit, height: height / fit });
  // Placed once the view is measured; until then nothing glides into place.
  let placed = $state(false);
  $effect(() => {
    if (placed || !width || !height) return;
    const next = requestAnimationFrame(() => (placed = true));
    return () => cancelAnimationFrame(next);
  });
  let sky = $derived(scene());

  let air = $state<HTMLCanvasElement>();
  let windowElement = $state<HTMLElement>();
  let flights = $state<HTMLElement>();

  let mind = $state<Mind | null>(null);
  let server = $state.raw<WorkLocalServerV1 | null>(null);
  let discovering = $state(true);
  let waitingForLocal = $derived(step === "ai" && mind === "local" && discovering);

  let kept = $derived(keptSites(tabs.sidebarNodes(), tabs.tabs(), SITE_URLS));
  let sites = $derived([...kept.keys()].flatMap((id) => SITES.filter((site) => site.id === id)));
  let imported = $derived.by(() => {
    const job = browserImport.current();
    if (!job?.finished) return null;
    const from = browserImport.found()?.find((source) => source.id === job.source)?.name ?? "";
    const bookmarks = job.kinds.find((entry) => entry.kind === "bookmarks")?.done ?? 0;
    return { from, bookmarks };
  });

  // Welcome is the window's new tab, so close that its sky is all there is;
  // every step after pulls back from it, and leaving goes back into it.
  let close = $derived(cover(view));
  let shot = $derived<Shot>(step === "welcome" || leaving ? close : SHOTS[step]);
  // The new tab's name, where welcome shows it: the dots gather there.
  let hero = $derived.by((): Box => {
    const [x, y] = land(close, [PAGE.x + NEWTAB.name.x, PAGE.y + NEWTAB.name.y]);
    return { x, y, w: NEWTAB.name.w * close.scale, h: NEWTAB.name.h * close.scale };
  });
  // The fade is masked one scene past the last shot that fades, by then fully
  // open, so taking the mask away never shows as a jump.
  let masked = $derived(
    step === "welcome" || step === "import" || step === "ai" || step === "ready",
  );
  // Work and its models are offered only while Work and AI are on.
  let ai = $derived(preferences.aiEnabled());
  let place = $derived(position(step, ai));

  // The name forms once the window is actually on screen and its sky in
  // place, never into a window native has not revealed yet.
  let revealed = $derived(motion.launchState() !== "armed" && placed);
  // Started by the reveal alone: nothing it reads later, the stage's scale
  // included, may restart or cut short a formation already under way.
  $effect(() => {
    if (!revealed || !air) return;
    return untrack(() => {
      if (phase !== "waiting" || !air) return;
      phase = "forming";
      void commands.onboardingPlayIntro().catch(() => {});
      if (reducedMotion()) {
        phase = "still";
        return;
      }
      const color = getComputedStyle(air).getPropertyValue("--sky-ink");
      const box = hero;
      return formName(air, {
        stage: STAGE,
        box,
        path: new Path2D(WORDMARK.d),
        place: (context) => {
          context.scale(box.w / WORDMARK.width, box.h / WORDMARK.height);
          context.translate(-WORDMARK.x, -WORDMARK.y);
          context.scale(WORDMARK.scaleX, WORDMARK.scaleY);
        },
        density: Math.min(2, window.devicePixelRatio || 1) * Math.min(1.5, fit),
        color: color.trim() || "#ffffff",
        onformed: () => (phase = "formed"),
        ondone: () => (phase = "still"),
      });
    });
  });

  function begin() {
    if (step !== "welcome" || (phase !== "formed" && phase !== "still")) return;
    step = "import";
  }

  function go(next: Step | undefined) {
    if (!next || leaving) return;
    step = next;
  }

  async function finish() {
    if (leaving || waitingForLocal) return;
    leaving = true;
    const profile = tabs.profile()?.id;
    // A server already running here is connected now, so Work opens ready.
    if (mind === "local") {
      if (!server || !profile) {
        leaving = false;
        say(m.onb_local_model_failed());
        return;
      }
      const saved = await commands.workSetModelEndpoint(profile, server.base).catch(() => null);
      if (!saved || saved.fault) {
        leaving = false;
        say(m.onb_local_model_failed());
        return;
      }
    }
    // The scene ends on the new tab, so the browser opens on one: a tab left
    // on a page from before is set aside for a new one.
    if (tabs.activeTab()?.url) tabs.open();
    // The words go, and the camera goes back into the new tab until its sky
    // is all there is; the browser then arrives on that sky, with its own
    // launch.
    await new Promise((resolve) => setTimeout(resolve, reducedMotion() ? 0 : 1500));
    if (await onfinish()) return;
    leaving = false;
    say(m.onb_finish_failed());
  }

  const primary = $derived.by((): string => {
    if (step === "import") return imported ? m.onb_continue() : m.onb_skip_step();
    if (step === "ai") return mind ? m.onb_continue() : m.onb_not_now();
    if (step === "ready") return m.onb_start();
    return m.onb_continue();
  });
  const advance = () => {
    if (waitingForLocal) return;
    if (step === "ready") void finish();
    else go(neighbour(step, 1, ai));
  };

  function keydown(event: KeyboardEvent) {
    if (event.key !== "Enter" || event.repeat || event.isComposing) return;
    if ((event.target as HTMLElement | null)?.closest("button, a, input")) return;
    event.preventDefault();
    if (step === "welcome") begin();
    else advance();
  }

  const headings: Partial<Record<Step, { title: () => string; body?: () => string }>> = {
    import: { title: m.onb_import_title, body: m.onb_import_body },
    essentials: { title: m.onb_essentials_title, body: m.onb_essentials_body },
    launcher: { title: m.onb_launcher_title, body: m.onb_launcher_body },
    work: { title: m.onb_work_title, body: m.onb_work_body },
    ai: { title: m.onb_ai_title, body: m.onb_ai_body },
    ready: { title: m.onb_ready_title },
  };

  // What is already on before anything is set: the reasons to stay.
  const features = [
    { icon: Shield01Icon, label: m.onb_ready_blocking },
    { icon: PuzzleIcon, label: m.onb_ready_extensions },
    { icon: CheckListIcon, label: m.onb_ready_tasks },
    { icon: Note01Icon, label: m.onb_ready_notes },
  ];

  function landing(site: string) {
    const target =
      windowElement?.querySelector(`[data-kept="${CSS.escape(site)}"]`) ??
      windowElement?.querySelector("[data-kept-more]");
    return target?.getBoundingClientRect() ?? null;
  }

  let noticeTimer: ReturnType<typeof setTimeout> | undefined;
  function say(text: string) {
    notice = text;
    clearTimeout(noticeTimer);
    noticeTimer = setTimeout(() => (notice = null), 3200);
  }
  const failed = (site: string) => say(m.onb_essentials_failed({ site }));

  onMount(() => {
    void browserImport.detect();
    return () => clearTimeout(noticeTimer);
  });
</script>

<svelte:window onkeydown={keydown} />

<div
  class="onboarding"
  role="dialog"
  aria-modal="true"
  aria-label={m.onb_label()}
  data-step={step}
  data-phase={phase}
  data-leaving={leaving}
  data-sky={sky}
  bind:clientWidth={width}
  bind:clientHeight={height}
>
  <div class="ground" data-tauri-drag-region></div>

  <div class="stage" style:transform={`translate(-50%, -50%) scale(${fit})`}>
    <div
      class="rig"
      style:transform={frame(shot)}
      style:--reach={`${shot.fade ?? 1200}px`}
      data-masked={masked}
      data-placed={placed}
    >
      <Window
        mode={step === "work" || step === "ai" ? "work" : "browse"}
        {sites}
        {imported}
        playing={step === "work"}
        modes={ai}
        scene={sky}
        skyZoom={fit * close.scale}
        space={tabs.activeSpace()?.name ?? ""}
        named={!leaving && (step !== "welcome" || phase === "formed" || phase === "still")}
        timed={!leaving && step !== "welcome"}
        bind:element={windowElement}
      />
    </div>

    {#if phase !== "still"}<canvas class="air" bind:this={air} aria-hidden="true"></canvas>{/if}

    <div class="welcome" style:top={`${hero.y + hero.h + 30}px`} data-glass-text>
      <p>{m.onb_tagline()}</p>
      <Button variant="primary" size="large" shape="capsule" onclick={begin}
        >{m.onb_begin()}<KeyHint keys={["↵"]} /></Button
      >
    </div>

    {#key step}
      {@const heading = headings[step]}
      {#if heading}
        <header class="head" data-glass-text>
          <h1>{heading.title()}</h1>
          {#if heading.body}<p>{heading.body()}</p>{/if}
          {#if step === "ready"}
            <ul class="features">
              {#each features as feature, index (index)}
                <li style:--i={index}><Icon icon={feature.icon} size={16} />{feature.label()}</li>
              {/each}
            </ul>
          {/if}
        </header>
      {/if}
      <div class="scene" data-scene={step}>
        {#if step === "import"}
          <Import />
        {:else if step === "essentials"}
          <Essentials {kept} {landing} flights={() => flights} onfailed={failed} />
        {:else if step === "launcher"}
          <Launcher />
        {:else if step === "ai"}
          <Models bind:mind bind:server bind:discovering onsubmit={advance} />
          {#if discovering}
            <p id="local-discovery-status" class="discovery" role="status">
              {m.onb_ai_discovering()}
            </p>
          {/if}
        {/if}
      </div>
    {/key}

    <nav class="bar" aria-label={m.onb_label()} data-glass-text>
      <span class="side">
        {#if neighbour(step, -1, ai)}<Button
            variant="ghost"
            size="large"
            shape="capsule"
            onclick={() => go(neighbour(step, -1, ai))}>{m.onb_back()}</Button
          >{/if}
      </span>
      <span class="middle">
        {#if notice}<span class="notice" role="status">{notice}</span>{:else}
          <ol aria-label={m.onb_progress({ current: place.at, total: place.of })}>
            {#each stepsFor(ai).slice(1) as candidate, index (candidate)}
              <li
                data-state={index + 1 < place.at ? "done" : index + 1 === place.at ? "now" : ""}
              ></li>
            {/each}
          </ol>
        {/if}
      </span>
      <span class="side end">
        <Button
          variant="primary"
          size="large"
          shape="capsule"
          onclick={advance}
          pending={leaving}
          aria-describedby={waitingForLocal ? "local-discovery-status" : undefined}
          disabled={waitingForLocal}>{primary}<KeyHint keys={["↵"]} /></Button
        >
      </span>
    </nav>
  </div>

  <div class="top" data-tauri-drag-region data-native-caption={IS_WINDOWS || undefined}>
    {#if step !== "welcome" && step !== "ready"}
      <button
        type="button"
        class="skip"
        disabled={waitingForLocal}
        aria-describedby={waitingForLocal ? "local-discovery-status" : undefined}
        onclick={() => void finish()}>{m.onb_skip()}</button
      >
    {/if}
    {#if !IS_MAC && windowControls}{@render windowControls()}{/if}
  </div>

  <!-- Marks thrown into the window's dock fly above the stage. -->
  <div class="flights" bind:this={flights} aria-hidden="true"></div>
</div>

<style>
  .onboarding {
    --camera: 1400ms cubic-bezier(0.7, 0, 0.12, 1);

    position: fixed;
    inset: 0;
    overflow: hidden;
    color: var(--color-text);
    font-family: var(--font-sans);
  }

  /* Nothing drawn: the window's material, or its canvas, is the ground. */
  .ground {
    position: absolute;
    inset: 0;
  }

  .stage {
    position: absolute;
    inset-block-start: 50%;
    inset-inline-start: 50%;
    inline-size: 1240px;
    block-size: 780px;
    transform-origin: center;
  }

  .air {
    position: absolute;
    inset: 0;
    inline-size: 1240px;
    block-size: 780px;
    pointer-events: none;
  }

  /* Under the name, in the sky: words in the sky's ink, and a control
     filled with it. */
  .welcome {
    --color-lit: var(--sky-ink);
    --color-lit-hover: var(--sky-ink-hover);
    --color-on-lit: var(--sky-on-ink);

    position: absolute;
    inset-inline: 0;
    display: grid;
    justify-items: center;
    gap: 28px;
    pointer-events: none;
  }

  .welcome p {
    max-inline-size: 30ch;
    margin: 0;
    color: var(--sky-ink);
    font-size: 17px;
    font-weight: 500;
    line-height: 1.5;
    text-align: center;
    text-shadow:
      0 0 1px var(--sky-type-edge),
      0 1px 18px var(--sky-type-halo);
    text-wrap: balance;
  }

  .welcome > :global(*) {
    opacity: 0;
    transform: translateY(8px);
    transition:
      opacity 1000ms var(--ease-out),
      transform 1000ms var(--ease-emphasized);
  }

  .welcome > :global(:last-child) {
    transition-delay: 220ms;
  }

  .welcome :global(.ui-button) {
    gap: 12px;
    padding-inline: 20px 11px;
  }

  .onboarding[data-step="welcome"]:is([data-phase="formed"], [data-phase="still"]) .welcome {
    pointer-events: auto;
  }

  .onboarding[data-step="welcome"]:is([data-phase="formed"], [data-phase="still"])
    .welcome
    > :global(*) {
    opacity: 1;
    transform: none;
  }

  .onboarding:not([data-step="welcome"]) .welcome > :global(*) {
    transition-duration: 300ms;
    transition-delay: 0ms;
  }

  /* The window, framed for each scene by moving and scaling it, never by
     changing its layout. */
  .rig {
    position: absolute;
    inset-block-start: 0;
    inset-inline-start: 0;
    inline-size: 880px;
    block-size: 560px;
    transition:
      transform var(--camera),
      --reach var(--camera),
      opacity 900ms var(--ease-out);
  }

  /* While it waits below the opening scenes, it fades into the ground. */
  .rig[data-masked="true"] {
    mask-image: linear-gradient(
      to bottom,
      black calc(var(--reach) - 180px),
      transparent var(--reach)
    );
  }

  .rig[data-placed="false"] {
    transition: none;
  }

  @property --reach {
    syntax: "<length>";
    inherits: false;
    initial-value: 1200px;
  }

  .head {
    position: absolute;
    inset-block-start: 74px;
    inset-inline: 0;
    display: grid;
    justify-items: center;
    gap: 12px;
    text-align: center;
    pointer-events: none;
  }

  .head h1 {
    margin: 0;
    font-size: 40px;
    font-weight: 500;
    line-height: 1.1;
    letter-spacing: -0.032em;
  }

  .head p {
    max-inline-size: 52ch;
    margin: 0;
    color: var(--color-muted);
    font-size: 15px;
    line-height: 1.5;
    text-wrap: balance;
  }

  .head > * {
    animation: rise 900ms var(--ease-emphasized) 250ms backwards;
  }

  .head > p {
    animation-delay: 360ms;
  }

  .features {
    display: flex;
    gap: 30px;
    margin: 8px 0 0;
    padding: 0;
    list-style: none;
    animation: none;
  }

  .features li {
    display: flex;
    align-items: center;
    gap: 8px;
    color: var(--color-muted);
    font-size: 14px;
    font-weight: 500;
    white-space: nowrap;
    animation: rise 800ms var(--ease-emphasized) backwards;
    animation-delay: calc(380ms + var(--i) * 70ms);
  }

  .features li :global(svg) {
    color: var(--color-text);
  }

  .scene {
    position: absolute;
    inset-inline: 0;
    display: grid;
    justify-items: center;
    animation: rise 900ms var(--ease-emphasized) 420ms backwards;
  }

  .scene[data-scene="you"] {
    inset-block-start: 236px;
  }

  /* Beside the window, centred on it however many browsers were found. */
  .scene[data-scene="import"] {
    inset-block-start: 272px;
    inset-inline: 832px auto;
    block-size: 392px;
    align-content: center;
  }

  .scene[data-scene="essentials"] {
    inset-block-start: 236px;
    inset-inline: 142px auto;
  }

  .scene[data-scene="launcher"] {
    inset-block-start: 226px;
  }

  .scene[data-scene="ai"] {
    inset-block-start: 236px;
  }

  .discovery {
    margin: 16px 0 0;
    color: var(--color-muted);
    font-size: var(--text-label);
  }

  /* One quiet row for the whole journey, at the foot of the stage: nothing
     floats over the scene it steers. */
  .bar {
    position: absolute;
    inset-block-end: 34px;
    inset-inline: 56px;
    display: grid;
    grid-template-columns: 1fr auto 1fr;
    align-items: center;
    block-size: 44px;
    transition:
      opacity 600ms var(--ease-out),
      transform 600ms var(--ease-emphasized);
  }

  .onboarding:is([data-step="welcome"], [data-leaving="true"]) .bar {
    opacity: 0;
    pointer-events: none;
    transform: translateY(12px);
  }

  .side {
    display: flex;
  }

  .side.end {
    justify-content: flex-end;
  }

  .side.end :global(.ui-button) {
    gap: 12px;
    min-inline-size: 132px;
    padding-inline: 20px 12px;
  }

  /* On the lit fill a control-grey key vanishes; it takes the label's ink. */
  .onboarding :global(.ui-button[data-variant="primary"] .ui-keys kbd) {
    background: color-mix(in srgb, var(--color-on-lit) 10%, transparent);
    color: color-mix(in srgb, var(--color-on-lit) 64%, transparent);
  }

  .middle ol {
    display: flex;
    gap: 5px;
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .middle li {
    inline-size: 18px;
    block-size: 3px;
    border-radius: var(--radius-capsule);
    background: var(--color-fill-strong);
    transition:
      inline-size var(--motion-page) var(--ease-emphasized),
      background-color var(--motion-slow) var(--ease-out);
  }

  .middle li[data-state="done"] {
    background: var(--color-faint);
  }

  .middle li[data-state="now"] {
    inline-size: 34px;
    background: var(--color-text);
  }

  .notice {
    color: var(--color-danger);
    font-size: 12.5px;
  }

  .top {
    position: absolute;
    inset-block-start: 0;
    inset-inline: 0;
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 8px;
    block-size: 52px;
    padding-inline-end: 20px;
  }

  /* Windows keeps its own caption buttons in this corner, 3 x 44 + 2 x 3
     wide, and their input host takes every click over them. */
  .top[data-native-caption] {
    padding-inline-end: calc(3 * 44px + 2 * 3px + 12px);
  }

  .skip {
    padding: 6px 10px;
    border: 0;
    border-radius: var(--radius-control-compact);
    background: transparent;
    color: var(--color-faint);
    font: inherit;
    font-size: var(--text-label);
    font-weight: 500;
    cursor: default;
    transition:
      color var(--motion-instant) var(--ease-smooth),
      background-color var(--motion-instant) var(--ease-smooth);
  }

  .skip:hover {
    background: var(--color-fill);
    color: var(--color-muted);
  }

  /* Leaving: the words go first, then the camera goes back into the new
     tab, and the browser arrives on its sky. */
  .onboarding[data-leaving="true"] :is(.head, .scene, .top) {
    opacity: 0;
    transition: opacity 360ms var(--ease-exit);
  }

  .flights {
    position: absolute;
    inset: 0;
    pointer-events: none;
  }

  .flights :global(.flight) {
    position: fixed;
    object-fit: contain;
    will-change: transform, opacity;
  }

  @keyframes rise {
    from {
      opacity: 0;
      transform: translateY(10px);
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .rig,
    .bar,
    .welcome > :global(*) {
      transition: none;
    }

    .head > *,
    .features li,
    .scene {
      animation: none;
    }
  }
</style>
