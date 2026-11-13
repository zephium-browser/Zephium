<!--
  Stands where a shut page would be while a focus round runs. The page itself
  is off the stage and its media held still; this says why, how long, and
  offers the two honest ways out.
-->
<script lang="ts">
  import { onMount } from "svelte";
  import { Globe02Icon } from "@hugeicons/core-free-icons";
  import * as m from "$shared/i18n/messages";
  import { favicons } from "$domain/favicons";
  import { countdown, focus } from "$domain/time";
  import Button from "$shared/ui/Button";
  import FavIcon from "$shared/ui/FavIcon";
  import Ring from "$shared/ui/data/Ring";
  import { everyWhileVisible } from "$shared/lib/visible-interval";

  let { site }: { site: string } = $props();

  let now = $state(Date.now());
  let confirming = $state(false);
  let session = $derived(focus.session());
  let mark = $derived(favicons.mark(null, `https://${site}/`));

  onMount(() => everyWhileVisible(1000, () => (now = Date.now())));

  function end() {
    if (!confirming) {
      confirming = true;
      return;
    }
    confirming = false;
    void focus.stop();
  }
</script>

<section class="library-shell cover" aria-labelledby="focus-cover-title">
  <div class="center">
    {#if session}
      <Ring value={1 - focus.progress(session, now)} size={176} stroke={8}>
        <span class="clock">{countdown(focus.remaining(session, now))}</span>
        <span class="left">{m.focus_cover_left()}</span>
      </Ring>
    {/if}
    <span class="site"
      ><FavIcon
        image={mark?.image ?? null}
        tone={mark?.tone}
        size={16}
        lit
        fallback={Globe02Icon}
      />{site}</span
    >
    <h1 id="focus-cover-title">{m.focus_cover_title()}</h1>
    <p>{m.focus_cover_body({ site })}</p>
    <div class="actions">
      <Button shape="capsule" pending={focus.busy()} onclick={() => void focus.allow(site)}
        >{m.focus_cover_allow()}</Button
      >
      <Button
        shape="capsule"
        variant={confirming ? "danger" : "ghost"}
        onblur={() => (confirming = false)}
        onclick={end}>{confirming ? m.focus_end_confirm() : m.focus_cover_end()}</Button
      >
    </div>
    {#if focus.lastFailed()}<p class="failed" role="alert">{m.focus_failed()}</p>{/if}
  </div>
</section>

<style>
  .cover {
    display: grid;
    place-items: center;
  }

  .center {
    display: grid;
    justify-items: center;
    gap: 10px;
    max-inline-size: 420px;
    padding: 32px;
    text-align: center;
    animation: arrive var(--motion-page) var(--ease-emphasized);
  }

  .clock {
    font-size: 32px;
    font-weight: 600;
    font-variant-numeric: tabular-nums;
    letter-spacing: -0.02em;
  }

  .left {
    color: var(--color-muted);
    font-size: var(--text-caption);
  }

  .site {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    margin-block-start: 18px;
    padding: 4px 10px 4px 6px;
    border-radius: var(--radius-capsule);
    background: var(--color-fill);
    color: var(--color-label-secondary);
    font-size: var(--text-label);
  }

  h1 {
    margin: 4px 0 0;
    font-size: var(--text-title);
    font-weight: 600;
    letter-spacing: -0.01em;
  }

  p {
    margin: 0;
    color: var(--color-muted);
    font-size: var(--text-body);
  }

  .actions {
    display: flex;
    flex-wrap: wrap;
    justify-content: center;
    gap: 8px;
    margin-block-start: 14px;
  }

  .failed {
    color: var(--color-danger);
    font-size: var(--text-label);
  }

  @keyframes arrive {
    from {
      opacity: 0;
      transform: translateY(8px);
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .center {
      animation: none;
    }
  }

  :global(:root[data-reduce-motion="true"]) .center {
    animation: none;
  }
</style>
