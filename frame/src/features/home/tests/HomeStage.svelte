<script lang="ts">
  import {
    Search01Icon,
    Settings01Icon,
    Shield01Icon,
    Clock01Icon,
    CheckListIcon,
    Home01Icon,
  } from "@hugeicons/core-free-icons";
  import Icon from "$shared/ui/Icon";
  import { WORDMARK } from "$shared/lib/wordmark";
  import type { ComponentProps } from "svelte";
  import HomeFrame from "../components/HomeFrame.svelte";
  import type { HomeIntent, HomeModel } from "../lib/model";
  import { mark, picture } from "./pictures";

  let {
    home,
    onintent = () => {},
    backdrop = false,
    load,
    onready,
  }: {
    home: HomeModel;
    onintent?: (intent: HomeIntent) => void;
    /** A chosen background picture under the page. */
    backdrop?: boolean;
    load?: ComponentProps<typeof HomeFrame>["load"];
    onready?: (enter: () => void) => void;
  } = $props();
  const letters = `scale(${280 / WORDMARK.width}) translate(${-WORDMARK.x} ${-WORDMARK.y}) scale(${WORDMARK.scaleX} ${WORDMARK.scaleY})`;
</script>

<!-- Stand-ins for what Home's next stage takes from the window, the new tab and the field. -->
<div class="window">
  <nav class="side" aria-label="Side">
    <span class="lights"><i></i><i></i><i></i></span>
    <span class="item on"><Icon icon={Home01Icon} size={15} />Home</span>
    <span class="group">stripe pricing</span>
    <span class="item"
      ><i class="dot" style:--hue="var(--color-tint-lavender)"></i>Pricing &amp; fees | Stripe</span
    >
    <span class="item"><i class="dot" style:--hue="var(--color-tint-sky)"></i>Stripe Docs</span>
    <span class="item"
      ><i class="dot" style:--hue="var(--color-tint-graphite)"></i>Pull requests · GitHub</span
    >
  </nav>
  <main class="page" class:backdrop>
    <HomeFrame {home} {onintent} {picture} {mark} {load} {onready}>
      {#snippet ground()}{#if backdrop}<div class="picture"></div>{/if}{/snippet}
      {#snippet field()}
        <div class="notch">
          <label class="search"
            ><Icon icon={Search01Icon} size={15} /><input
              placeholder="Search or enter an address"
              aria-label="Search or enter an address"
            /></label
          >
        </div>
      {/snippet}
      {#snippet welcome()}
        <div class="welcome">
          <svg
            role="img"
            aria-label="Zephium"
            width="280"
            height={280 * (WORDMARK.height / WORDMARK.width)}
            ><path d={WORDMARK.d} transform={letters} /></svg
          >
          <p>2:32 PM · Monday, November 23</p>
        </div>
      {/snippet}
      {#snippet controls()}<span class="disc"><Icon icon={Settings01Icon} size={16} /></span
        >{/snippet}
      {#snippet figures()}
        <div class="figures">
          <span><Icon icon={Shield01Icon} size={14} />Ads &amp; trackers blocked <b>1,284</b></span>
          <span><Icon icon={Clock01Icon} size={14} />Focused <b>2h 14m</b></span>
          <span><Icon icon={CheckListIcon} size={14} />Due today <b>3</b></span>
        </div>
      {/snippet}
    </HomeFrame>
  </main>
</div>

<style>
  .window {
    display: grid;
    grid-template-columns: 248px 1fr;
    inline-size: 100vw;
    block-size: 100vh;
    background: var(--color-chrome);
    color: var(--color-text);
    font-size: var(--text-body);
  }

  .side {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 14px 10px;
  }

  .lights {
    display: flex;
    gap: 8px;
    padding: 2px 6px 18px;
  }

  .lights i {
    inline-size: 12px;
    block-size: 12px;
    border-radius: 50%;
    background: var(--color-fill-strong);
  }

  .item {
    display: flex;
    align-items: center;
    gap: 9px;
    block-size: 32px;
    padding-inline: 10px;
    overflow: hidden;
    border-radius: var(--radius-row);
    color: var(--color-label-secondary);
    white-space: nowrap;
  }

  .item.on {
    background: var(--color-fill-active);
    color: var(--color-text);
    font-weight: 600;
  }

  .group {
    padding: 14px 10px 4px;
    color: var(--color-faint);
    font-size: var(--text-caption);
    font-weight: 600;
  }

  .dot {
    flex: none;
    inline-size: 14px;
    block-size: 14px;
    border-radius: 4px;
    background: var(--hue);
  }

  .page {
    margin: 8px 8px 8px 0;
    overflow: hidden;
    border-radius: var(--content-radius);
  }

  .picture {
    position: absolute;
    inset: 0;
    background:
      radial-gradient(120% 90% at 20% 10%, var(--color-soft-peach), transparent 60%),
      radial-gradient(90% 80% at 85% 30%, var(--color-soft-lilac), transparent 65%),
      radial-gradient(120% 100% at 50% 110%, var(--color-soft-sky), transparent 70%),
      var(--color-soft-mint);
  }

  .notch {
    position: relative;
    display: grid;
    place-items: center;
    inline-size: 640px;
    block-size: 50px;
    margin-inline: auto;
    border-end-start-radius: 18px;
    border-end-end-radius: 18px;
    background: var(--color-chrome);
  }

  .notch::before,
  .notch::after {
    position: absolute;
    inset-block-start: 0;
    inline-size: 14px;
    block-size: 14px;
    content: "";
  }

  .notch::before {
    inset-inline-start: -14px;
    background: radial-gradient(circle at 0 100%, transparent 13.5px, var(--color-chrome) 14px);
  }

  .notch::after {
    inset-inline-end: -14px;
    background: radial-gradient(circle at 100% 100%, transparent 13.5px, var(--color-chrome) 14px);
  }

  .search {
    display: flex;
    align-items: center;
    gap: 8px;
    inline-size: 600px;
    block-size: 36px;
    padding-inline: 14px;
    border-radius: var(--radius-control-large);
    background: var(--color-address);
    color: var(--color-faint);
  }

  input {
    flex: 1;
    border: 0;
    outline: 0;
    background: none;
    color: var(--color-text);
    font: inherit;
  }

  .welcome {
    display: grid;
    justify-items: center;
    gap: 14px;
    color: var(--color-text);
  }

  .welcome path {
    fill: currentcolor;
  }

  .welcome p {
    margin: 0;
    color: var(--color-muted);
    font-size: 16px;
  }

  .disc {
    display: grid;
    place-items: center;
    inline-size: 32px;
    block-size: 32px;
    border-radius: 50%;
    background: var(--color-fill);
    color: var(--color-muted);
  }

  .figures {
    display: flex;
    gap: 8px;
  }

  .figures span {
    display: flex;
    align-items: center;
    gap: 7px;
    block-size: 34px;
    padding: 0 14px 0 12px;
    border-radius: var(--radius-capsule);
    background: var(--color-surface);
    color: var(--color-muted);
    font-size: 12.5px;
    font-weight: 500;
  }

  .figures b {
    color: var(--color-text);
    font-weight: 600;
  }
</style>
