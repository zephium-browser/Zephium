<script lang="ts">
  import {
    ArrowDown01Icon,
    ArrowLeft02Icon,
    ArrowRight02Icon,
    Briefcase02Icon,
    Folder01Icon,
    Globe02Icon,
    Layers01Icon,
    PlusSignIcon,
    Refresh01Icon,
    Search01Icon,
    SidebarLeftIcon,
    UserIcon,
  } from "@hugeicons/core-free-icons";
  import * as m from "$shared/i18n/messages";
  import Icon from "$shared/ui/Icon";
  import { WORDMARK } from "$shared/lib/wordmark";
  import { PICTURE, placeSky, SkyPrint, type Scene } from "$shared/sky";
  import { NEWTAB, PAGE } from "../lib/camera";
  import type { Site } from "../lib/catalog";
  import Mark from "$shared/ui/BrandMark";
  import WorkCanvas from "./WorkCanvas.svelte";

  let {
    mode,
    sites,
    imported,
    playing,
    modes = true,
    scene,
    skyZoom = 1,
    space,
    named = true,
    timed = true,
    element = $bindable(),
  }: {
    mode: "browse" | "work";
    /** The Browse and Work switch, absent while Work and AI are off. */
    modes?: boolean;
    /** What the person has kept, in sidebar order. */
    sites: Site[];
    /** What an import brought, once it has. */
    imported: { from: string; bookmarks: number } | null;
    /** The Work run plays only while it is on screen. */
    playing: boolean;
    /** The new tab's sky. */
    scene: Scene;
    /** How far the camera ever closes in on the page. */
    skyZoom?: number;
    /** The name of the space the tabs are in. */
    space: string;
    /** Whether the new tab's name and its time line show. */
    named?: boolean;
    timed?: boolean;
    element?: HTMLElement;
  } = $props();

  // One row of the dock beside its shelf: the rest are counted, not crammed.
  let shown = $derived(sites.length > 3 ? sites.slice(0, 2) : sites);
  let more = $derived(sites.length > 3 ? sites.length - 2 : 0);
  let now = $state(new Date());
  $effect(() => {
    const timer = setInterval(() => (now = new Date()), 30_000);
    return () => clearInterval(timer);
  });
  let time = $derived(now.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" }));
  let day = $derived(
    now.toLocaleDateString(undefined, { weekday: "long", month: "long", day: "numeric" }),
  );
  const number = new Intl.NumberFormat();

  const sky = placeSky(PAGE.width, NEWTAB.horizon);
  const letters =
    `scale(${NEWTAB.name.w / WORDMARK.width}) ` +
    `translate(${-WORDMARK.x} ${-WORDMARK.y}) scale(${WORDMARK.scaleX} ${WORDMARK.scaleY})`;
</script>

<!-- The browser itself, drawn small from the same tokens it is built from:
     the window onboarding shows is the window it hands over to. -->
<div class="window" data-mode={mode} bind:this={element} aria-hidden="true">
  <aside class="side">
    <div class="browse">
      <div class="top">
        <span class="lights"><i></i><i></i><i></i></span>
        <Icon icon={SidebarLeftIcon} size={14} />
        <span class="gap"></span>
        <Icon icon={ArrowLeft02Icon} size={14} />
        <Icon icon={ArrowRight02Icon} size={14} />
        <Icon icon={Refresh01Icon} size={14} />
      </div>
      {#if modes}<div class="modes">
          <span class="on"><Icon icon={Globe02Icon} size={13} />{m.mode_browse()}</span>
          <span><Icon icon={Briefcase02Icon} size={13} />{m.mode_work()}</span>
        </div>{/if}
      <div class="address">{m.search_web_short()}</div>
      <div class="space">
        <span>{space}</span><Icon icon={ArrowDown01Icon} size={11} strokeWidth={2} />
        <span class="gap"></span>
        <Icon icon={PlusSignIcon} size={14} />
      </div>
      {#if imported}
        <div class="row folder" data-imported>
          <Icon icon={Folder01Icon} size={14} />{m.onb_window_from({ browser: imported.from })}
        </div>
        <div class="row quiet child">
          {m.onb_window_bookmarks({ count: number.format(imported.bookmarks) })}
        </div>
      {/if}
      <div class="row current"><Icon icon={Globe02Icon} size={14} />{m.new_tab()}</div>
      <div class="dock" data-dock>
        <span class="avatar"><Icon icon={UserIcon} size={15} /></span>
        <i class="rule"></i>
        {#if sites.length === 0}<span class="empty">{m.essential_drop_hint()}</span>{/if}
        {#each shown as site (site.id)}
          <span class="tile site" data-kept={site.id}><Mark mark={site.mark} size={17} /></span>
        {/each}
        {#if more}<span class="tile count" data-kept-more>+{more}</span>{/if}
      </div>
    </div>

    <div class="rail">
      <span class="lights"><i></i><i></i><i></i></span>
      <div class="rail-modes">
        <span><Icon icon={Globe02Icon} size={12} /></span>
        <span class="on"><Icon icon={Briefcase02Icon} size={12} /></span>
      </div>
      <span class="rail-icon"><Icon icon={Search01Icon} size={15} /></span>
      <hr />
      <span class="rail-icon"><Icon icon={Layers01Icon} size={15} /></span>
      <span class="rail-icon current"><Icon icon={Globe02Icon} size={15} /></span>
      <span class="rail-icon"><Icon icon={PlusSignIcon} size={15} /></span>
      <span class="gap"></span>
      {#each sites.slice(0, 3) as site (site.id)}
        <span class="rail-disc"><Mark mark={site.mark} size={16} /></span>
      {/each}
      <span class="rail-disc"><Icon icon={UserIcon} size={14} /></span>
    </div>
  </aside>

  <main class="pane">
    <!-- The new tab as it opens: the sky, the field hanging in its notch,
         the name low in the sky and the time under it. -->
    <div class="newtab" data-sky={scene}>
      <div class="sky">
        <SkyPrint
          {scene}
          width={PAGE.width}
          height={PAGE.height}
          {sky}
          horizon={NEWTAB.horizon}
          pictureHorizon={PICTURE.horizon}
          zoom={skyZoom}
        />
      </div>
      <div
        class="search"
        style:width={`${NEWTAB.notch.width}px`}
        style:height={`${NEWTAB.notch.height}px`}
      >
        <Icon icon={Search01Icon} size={13} />{m.search_web_short()}
      </div>
      <svg
        class="mark"
        data-shown={named}
        style:left={`${NEWTAB.name.x}px`}
        style:top={`${NEWTAB.name.y}px`}
        style:width={`${NEWTAB.name.w}px`}
        style:height={`${NEWTAB.name.h}px`}><path d={WORDMARK.d} transform={letters} /></svg
      >
      <p class="when" data-shown={timed} style:top={`${NEWTAB.when}px`}>
        <b>{time}</b><span>{day}</span>
      </p>
    </div>
    <div class="work">
      {#if mode === "work"}<WorkCanvas {playing} />{/if}
    </div>
  </main>
</div>

<style>
  .window {
    --side: 232px;

    position: absolute;
    inset: 0;
    display: grid;
    grid-template-columns: var(--side) 1fr;
    gap: 8px;
    padding: 8px;
    box-sizing: border-box;
    overflow: hidden;
    border-radius: var(--radius-card);
    background: var(--color-chrome);
    color: var(--color-text);
    box-shadow:
      var(--shadow-overlay),
      inset 0 0 0 0.5px var(--color-border-strong);
    font-family: var(--font-sans);
    transition: grid-template-columns 820ms var(--ease-emphasized);
  }

  .window[data-mode="work"] {
    --side: 52px;
  }

  .side {
    position: relative;
    min-inline-size: 0;
  }

  .browse,
  .rail {
    position: absolute;
    inset: 0;
    display: flex;
    flex-direction: column;
    transition: opacity 420ms var(--ease-out);
  }

  .browse {
    gap: 6px;
    inline-size: 232px;
  }

  .rail {
    align-items: center;
    gap: 8px;
    inline-size: 52px;
    padding-block-end: 2px;
    opacity: 0;
  }

  .window[data-mode="work"] .browse {
    opacity: 0;
  }

  .window[data-mode="work"] .rail {
    opacity: 1;
  }

  .top {
    display: flex;
    align-items: center;
    gap: 10px;
    block-size: 28px;
    padding-inline: 4px;
    color: var(--color-faint);
  }

  .lights {
    display: flex;
    gap: 6px;
    margin-inline-end: 4px;
  }

  .lights i {
    inline-size: 10px;
    block-size: 10px;
    border-radius: var(--radius-capsule);
    background: var(--color-fill-strong);
  }

  .gap {
    flex: 1;
  }

  .modes {
    display: grid;
    grid-template-columns: 1fr 1fr;
    block-size: 30px;
    padding: 2px;
    border-radius: var(--radius-control-compact);
    background: var(--color-fill);
    font-size: 12px;
    font-weight: 500;
  }

  .modes span {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
    border-radius: var(--radius-inset);
    color: var(--color-faint);
  }

  .modes .on {
    background: var(--color-raise-active);
    box-shadow: var(--shadow-raise);
    color: var(--color-text);
  }

  /* The head of the tab list: the space's name, and the plus that opens a
     tab in it. */
  .space {
    display: flex;
    align-items: center;
    gap: 6px;
    block-size: 30px;
    padding-inline: 9px 10px;
    color: var(--color-label-secondary);
    font-size: 12.5px;
    font-weight: 600;
    letter-spacing: -0.01em;
  }

  .space :global(svg) {
    color: var(--color-faint);
  }

  .address {
    display: grid;
    place-items: center;
    block-size: 30px;
    border-radius: var(--radius-control-compact);
    background: var(--color-fill);
    color: var(--color-muted);
    font-size: 12px;
  }

  .row {
    display: flex;
    align-items: center;
    gap: 9px;
    block-size: 30px;
    padding-inline: 9px;
    border-radius: var(--radius-inset);
    font-size: 12.5px;
    font-weight: var(--sidebar-row-weight);
    white-space: nowrap;
    color: var(--color-label-secondary);
  }

  .row :global(svg) {
    flex: none;
    color: var(--color-faint);
  }

  .row.quiet {
    color: var(--color-faint);
  }

  .row.current {
    margin-block-start: 4px;
    background: var(--row-active);
    box-shadow: var(--row-rim);
    color: var(--color-text);
  }

  .row.folder {
    animation: row-in 600ms var(--ease-emphasized) backwards;
  }

  .row.child {
    padding-inline-start: 32px;
    animation: row-in 600ms var(--ease-emphasized) 120ms backwards;
  }

  /* The profile, a rule, then what is kept, as the real foot is. */
  .dock {
    display: grid;
    grid-template-columns: 36px 1px repeat(3, 1fr);
    align-items: center;
    gap: 6px;
    margin-block-start: auto;
  }

  .avatar {
    display: grid;
    place-items: center;
    inline-size: 36px;
    block-size: 36px;
    border-radius: var(--radius-capsule);
    background: var(--color-card);
    color: var(--color-muted);
  }

  .rule {
    block-size: 22px;
    background: var(--color-border);
  }

  /* Nothing kept yet: the real dock's own invitation. */
  .empty {
    display: grid;
    grid-column: 3 / -1;
    place-items: center;
    block-size: 40px;
    padding-inline: 8px;
    border-radius: var(--radius-row);
    background: var(--color-tile);
    color: var(--color-faint);
    font-size: 10px;
    line-height: 1.3;
    text-align: center;
  }

  .tile {
    display: grid;
    place-items: center;
    block-size: 40px;
    border-radius: var(--radius-row);
    background: var(--color-card);
    color: var(--color-muted);
    font-size: 11.5px;
    font-weight: 600;
    font-variant-numeric: tabular-nums;
  }

  .tile.site,
  .tile.count {
    animation: tile-in 520ms var(--ease-snap) backwards;
  }

  .rail .lights {
    gap: 5px;
    block-size: 28px;
    margin: 0;
    align-items: center;
  }

  .rail .lights i {
    inline-size: 9px;
    block-size: 9px;
  }

  .rail-modes {
    display: flex;
    gap: 1px;
    padding: 2px;
    border-radius: var(--radius-capsule);
    background: var(--color-fill);
  }

  .rail-modes span {
    display: grid;
    place-items: center;
    inline-size: 21px;
    block-size: 20px;
    border-radius: var(--radius-capsule);
    color: var(--color-faint);
  }

  .rail-modes .on {
    background: var(--color-raise-active);
    color: var(--color-text);
  }

  .rail-icon {
    display: grid;
    place-items: center;
    inline-size: 36px;
    block-size: 36px;
    border-radius: var(--radius-control-compact);
    color: var(--color-muted);
  }

  .rail-icon.current {
    background: var(--row-active);
    box-shadow: var(--row-rim);
    color: var(--color-text);
  }

  .rail .gap {
    flex: 1;
  }

  .rail-disc {
    display: grid;
    place-items: center;
    inline-size: 34px;
    block-size: 34px;
    flex: none;
    border-radius: var(--radius-capsule);
    background: var(--color-card);
    color: var(--color-muted);
  }

  .rail hr {
    inline-size: 22px;
    margin: 2px 0;
    border: 0;
    border-block-start: 1px solid var(--color-border);
  }

  .pane {
    position: relative;
    overflow: hidden;
    border-radius: var(--content-radius);
    background: var(--color-page);
    box-shadow: inset 0 0 0 0.5px var(--color-border);
  }

  .newtab,
  .work {
    position: absolute;
    inset: 0;
    transition: opacity 520ms var(--ease-out);
  }

  .newtab {
    background: var(--sky-paper);
  }

  .work {
    opacity: 0;
  }

  .window[data-mode="work"] .newtab {
    opacity: 0;
  }

  .window[data-mode="work"] .work {
    opacity: 1;
  }

  .sky {
    position: absolute;
    inset: 0;
  }

  .search {
    position: absolute;
    inset-block-start: 0;
    inset-inline-start: 50%;
    display: flex;
    align-items: center;
    gap: 9px;
    padding-inline: 16px;
    box-sizing: border-box;
    transform: translateX(-50%);
    border-radius: 0 0 var(--radius-control) var(--radius-control);
    background: var(--color-chrome);
    box-shadow: 0 0.5px 0 var(--sky-lip);
    color: var(--color-faint);
    font-size: 12.5px;
  }

  .mark {
    position: absolute;
    overflow: visible;
    transition: opacity 900ms var(--ease-out);
  }

  .mark path {
    fill: var(--sky-name);
  }

  .when {
    position: absolute;
    inset-inline: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
    margin: 0;
    color: var(--sky-ink);
    font-size: 9px;
    font-variant-numeric: tabular-nums;
    text-shadow: 0 1px 10px var(--sky-shadow);
    transition: opacity 600ms var(--ease-out);
  }

  .when b {
    font-weight: 600;
  }

  .when span {
    opacity: 0.82;
  }

  .when span::before {
    content: "";
    display: inline-block;
    inline-size: 2px;
    block-size: 2px;
    margin-inline-end: 6px;
    border-radius: var(--radius-capsule);
    background: currentcolor;
    vertical-align: middle;
  }

  .when b,
  .when span {
    display: inline-flex;
    align-items: center;
  }

  :is(.mark, .when)[data-shown="false"] {
    opacity: 0;
    transition-duration: 360ms;
  }

  @keyframes row-in {
    from {
      opacity: 0;
      transform: translateX(-10px);
    }
  }

  @keyframes tile-in {
    from {
      opacity: 0;
      transform: scale(0.6);
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .window,
    .browse,
    .rail,
    .newtab,
    .work,
    .mark,
    .when {
      transition: none;
    }

    .row.folder,
    .row.child,
    .tile.site,
    .tile.count {
      animation: none;
    }
  }
</style>
