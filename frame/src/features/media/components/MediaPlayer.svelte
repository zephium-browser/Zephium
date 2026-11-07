<script lang="ts">
  import {
    Cancel01Icon,
    PictureInPictureOnIcon,
    VolumeHighIcon,
    VolumeOffIcon,
  } from "@hugeicons/core-free-icons";
  import * as m from "$shared/i18n/messages";
  import Icon from "$shared/ui/Icon";
  import FavIcon from "$shared/ui/FavIcon";
  import { favicons } from "$domain/favicons";
  import { mediaPlayer } from "$domain/media-player";
  import { tabs } from "$domain/tabs";
  import { clock, fraction } from "../lib/time";

  let { compact = false }: { compact?: boolean } = $props();

  // The disc is 36px and its ring a 1.5px stroke along its edge.
  const RING = 17.25;

  let player = $derived(mediaPlayer.view());
  // Native sends the position as of each change; between changes it is
  // arithmetic, redrawn once a second only while something plays.
  let now = $state(performance.now());
  $effect(() => {
    if (!player?.playing || player.waiting) return;
    now = performance.now();
    const tick = setInterval(() => (now = performance.now()), 1000);
    return () => clearInterval(tick);
  });
  // A seek holds its place until native reports the next state.
  let sought = $state.raw<{ for: object; seconds: number } | null>(null);
  let position = $derived(
    sought && sought.for === player ? sought.seconds : mediaPlayer.position(now),
  );
  let duration = $derived(player?.duration ?? null);
  let progress = $derived(fraction(position, duration));

  let hovered = $state(false);
  let focused = $state(false);
  let expanded = $derived(hovered || focused);

  function toggle() {
    if (player?.playing) mediaPlayer.pause();
    else mediaPlayer.play();
  }

  function seek(event: Event) {
    const seconds = Number((event.currentTarget as HTMLInputElement).value);
    if (!player || !Number.isFinite(seconds)) return;
    sought = { for: player, seconds };
    mediaPlayer.seek(seconds);
  }
</script>

{#snippet playGlyph(playing: boolean, size: number)}
  <svg width={size} height={size} viewBox="0 0 16 16" aria-hidden="true" class="media-glyph">
    {#if playing}
      <rect x="3.5" y="2.5" width="3" height="11" rx="1.1" />
      <rect x="9.5" y="2.5" width="3" height="11" rx="1.1" />
    {:else}
      <path
        d="M5 3.1v9.8a.9.9 0 0 0 1.37.77l7.6-4.9a.9.9 0 0 0 0-1.54l-7.6-4.9A.9.9 0 0 0 5 3.1Z"
      />
    {/if}
  </svg>
{/snippet}

{#if player}
  {#if compact}
    <!-- The rail has room for the site and what plays it: a plate like its
         other buttons, ringed by how far it has played from the top, which
         plays or pauses. -->
    <button
      type="button"
      class="media-disc"
      data-playing={player.playing || undefined}
      title={player.title}
      aria-label={`${player.playing ? m.media_pause() : m.media_play()}: ${player.title}`}
      onclick={toggle}
    >
      <svg class="media-ring" viewBox="0 0 36 36" aria-hidden="true">
        <circle class="media-ring-track" cx="18" cy="18" r={RING} />
        {#if duration !== null && progress > 0}<circle
            class="media-ring-fill"
            cx="18"
            cy="18"
            r={RING}
            pathLength="100"
            stroke-dasharray={`${progress * 100} 100`}
            transform="rotate(-90 18 18)"
          />{/if}
      </svg>
      <span class="media-disc-icon">
        <FavIcon
          image={favicons.image(player.icon)}
          tone={favicons.tone(player.icon)}
          size={18}
          lit
        />
      </span>
      <span class="media-disc-glyph">{@render playGlyph(player.playing, 14)}</span>
    </button>
  {:else}
    <section
      class="media-player"
      aria-label={m.media_player()}
      data-expanded={expanded || undefined}
      data-playing={player.playing || undefined}
      onpointerenter={() => (hovered = true)}
      onpointerleave={() => (hovered = false)}
      onfocusin={() => (focused = true)}
      onfocusout={(event) => {
        const next = event.relatedTarget as Node | null;
        if (!next || !(event.currentTarget as HTMLElement).contains(next)) focused = false;
      }}
    >
      <!-- The site's own colour, as an ambient glow behind the card. -->
      <span class="media-ambient" aria-hidden="true">
        <FavIcon image={favicons.image(player.icon)} size={56} lit />
      </span>
      <div class="media-head">
        <button
          type="button"
          class="media-site"
          aria-label={m.media_show_tab({ title: player.title })}
          title={m.media_show_tab({ title: player.title })}
          onclick={() => mediaPlayer.show()}
        >
          <FavIcon
            image={favicons.image(player.icon)}
            tone={favicons.tone(player.icon)}
            size={20}
            lit
          />
        </button>
        <div class="media-text">
          <span class="media-title" data-media-title>{player.title}</span>
          {#if player.artist}<span class="media-artist">{player.artist}</span>{/if}
        </div>
        <button
          type="button"
          class="media-control media-toggle"
          aria-label={player.playing ? m.media_pause() : m.media_play()}
          title={player.playing ? m.media_pause() : m.media_play()}
          onclick={toggle}
        >
          {@render playGlyph(player.playing, 14)}
        </button>
        {#if player.can_mute}<button
            type="button"
            class="media-control"
            data-muted={player.muted || undefined}
            aria-label={player.muted ? m.tab_unmute() : m.tab_mute()}
            aria-pressed={player.muted}
            title={player.muted ? m.tab_audio_muted() : m.tab_mute()}
            onclick={() => void tabs.setMuted(player!.tab_id, !player!.muted)}
          >
            <Icon icon={player.muted ? VolumeOffIcon : VolumeHighIcon} size={15} />
          </button>{/if}
      </div>
      <div class="media-more" inert={!expanded}>
        <div class="media-more-inner">
          {#if duration !== null}
            <input
              class="media-progress"
              type="range"
              min="0"
              max={duration}
              step="any"
              value={position}
              style:--progress={progress}
              aria-label={m.media_position()}
              aria-valuetext={m.media_position_text({
                position: clock(position),
                duration: clock(duration),
              })}
              onchange={seek}
            />
          {/if}
          <div class="media-foot">
            {#if duration !== null}
              <span class="media-time">{clock(position)}</span>
            {:else}
              <span class="media-live">{m.media_live()}</span>
            {/if}
            <span class="media-actions">
              {#if player.can_pip}<button
                  type="button"
                  class="media-control media-small"
                  aria-label={m.media_pip()}
                  aria-pressed={player.in_pip}
                  title={m.media_pip()}
                  onclick={() => mediaPlayer.pictureInPicture()}
                >
                  <Icon icon={PictureInPictureOnIcon} size={14} />
                </button>{/if}
              <button
                type="button"
                class="media-control media-small"
                aria-label={m.media_close()}
                title={m.media_close()}
                onclick={() => mediaPlayer.close()}
              >
                <Icon icon={Cancel01Icon} size={12} />
              </button>
            </span>
            {#if duration !== null}<span class="media-time"
                >-{clock(Math.max(0, duration - position))}</span
              >{/if}
          </div>
        </div>
      </div>
    </section>
  {/if}
{/if}

<style>
  .media-player {
    position: relative;
    isolation: isolate;
    flex: none;
    margin: 6px var(--sidebar-inset);
    padding: 8px 6px 8px 8px;
    overflow: hidden;
    border-radius: var(--radius-card);
    background: var(--row-active);
    box-shadow: var(--row-rim);
    animation: player-in var(--motion-base) var(--ease-emphasized) both;
  }

  /* The site's mark, blown up and blurred, tints the card with its colour
     the way album art tints a car's dashboard: drawn once, never animated. */
  .media-ambient {
    position: absolute;
    inset-block-start: -18px;
    inset-inline-start: -14px;
    z-index: -1;
    opacity: 0.32;
    filter: blur(22px) saturate(1.6);
    pointer-events: none;
    transition: opacity var(--motion-base) var(--ease-out);
  }

  .media-player:not([data-playing]) .media-ambient {
    opacity: 0.16;
  }

  .media-head {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .media-site {
    display: grid;
    flex: none;
    place-items: center;
    width: 30px;
    height: 30px;
    padding: 0;
    border: 0;
    border-radius: var(--radius-inset);
    background: var(--color-fill);
    cursor: default;
    transition:
      background-color var(--motion-fast) var(--ease-out),
      scale var(--motion-slow) var(--ease-spring);
  }

  .media-site:hover {
    background: var(--row-pressed);
  }

  .media-site:active {
    scale: 0.92;
    transition-duration: var(--motion-instant);
  }

  .media-text {
    display: grid;
    flex: 1;
    min-width: 0;
  }

  .media-title,
  .media-artist {
    overflow: hidden;
    white-space: nowrap;
    mask-image: var(--mask-fade-end);
  }

  .media-title {
    color: var(--color-text);
    font-size: var(--text-label);
    font-weight: 550;
  }

  .media-artist {
    color: var(--color-muted);
    font-size: var(--text-caption);
  }

  .media-title:dir(rtl),
  .media-artist:dir(rtl) {
    mask-image: var(--mask-fade-end-rtl);
  }

  .media-control {
    display: grid;
    flex: none;
    place-items: center;
    width: 28px;
    height: 28px;
    padding: 0;
    border: 0;
    border-radius: var(--radius-capsule);
    background: transparent;
    color: var(--color-label-secondary);
    cursor: default;
    transition:
      background-color var(--motion-fast) var(--ease-out),
      color var(--motion-fast) var(--ease-out),
      scale var(--motion-slow) var(--ease-spring);
  }

  .media-control:hover {
    background: var(--row-pressed);
    color: var(--color-text);
  }

  .media-control:active {
    scale: 0.88;
    transition-duration: var(--motion-instant);
  }

  .media-control:focus-visible,
  .media-site:focus-visible {
    outline: 2px solid var(--color-accent);
    outline-offset: 1px;
  }

  .media-toggle {
    color: var(--color-text);
  }

  .media-control[data-muted],
  .media-control[aria-pressed="true"]:not(.media-toggle) {
    color: var(--color-accent);
  }

  .media-small {
    width: 24px;
    height: 24px;
  }

  .media-glyph {
    fill: currentcolor;
  }

  /* The rest opens under the head while the pointer or focus is in the
     card, and folds away again: rows, not height, so nothing is measured. */
  .media-more {
    display: grid;
    grid-template-rows: 0fr;
    opacity: 0;
    transition:
      grid-template-rows var(--motion-base) var(--ease-emphasized),
      opacity var(--motion-fast) var(--ease-out);
  }

  .media-player[data-expanded] .media-more {
    grid-template-rows: 1fr;
    opacity: 1;
  }

  .media-more-inner {
    display: grid;
    gap: 2px;
    min-height: 0;
    overflow: hidden;
    padding-inline-end: 2px;
  }

  .media-progress {
    --track: var(--color-fill-strong);
    --fill: var(--color-text);

    appearance: none;
    width: 100%;
    height: 14px;
    margin: 8px 0 0;
    padding: 0;
    background: transparent;
    cursor: default;
  }

  .media-progress::-webkit-slider-runnable-track {
    height: 3px;
    border-radius: var(--radius-capsule);
    background: linear-gradient(
      to right,
      var(--fill) calc(var(--progress) * 100%),
      var(--track) calc(var(--progress) * 100%)
    );
    transition: height var(--motion-fast) var(--ease-out);
  }

  .media-progress:dir(rtl)::-webkit-slider-runnable-track {
    background: linear-gradient(
      to left,
      var(--fill) calc(var(--progress) * 100%),
      var(--track) calc(var(--progress) * 100%)
    );
  }

  .media-progress::-webkit-slider-thumb {
    appearance: none;
    width: 10px;
    height: 10px;
    margin-top: -3.5px;
    border-radius: var(--radius-capsule);
    background: var(--fill);
    box-shadow: var(--shadow-badge);
    scale: 0;
    transition: scale var(--motion-fast) var(--ease-out);
  }

  .media-progress:is(:hover, :focus-visible)::-webkit-slider-thumb {
    scale: 1;
  }

  .media-progress:focus-visible {
    outline: none;
  }

  .media-progress:focus-visible::-webkit-slider-runnable-track {
    box-shadow: 0 0 0 2px var(--color-accent);
  }

  .media-foot {
    display: flex;
    align-items: center;
    justify-content: space-between;
    min-height: 24px;
  }

  .media-time,
  .media-live {
    min-width: 38px;
    color: var(--color-muted);
    font-size: var(--text-caption);
    font-variant-numeric: tabular-nums;
  }

  .media-time:last-child {
    text-align: end;
  }

  .media-live {
    color: var(--color-accent);
    font-weight: 600;
  }

  .media-actions {
    display: flex;
    gap: 2px;
  }

  /* The rail: the site on the plate of the profile button below it, the same
     size, centred in the column and a rail step above the rule, ringed by how
     far it has played. */
  .media-disc {
    position: relative;
    display: grid;
    flex: none;
    place-items: center;
    align-self: center;
    width: 36px;
    height: 36px;
    margin-block-start: 8px;
    padding: 0;
    border: 0;
    border-radius: var(--radius-capsule);
    background: var(--row-active);
    box-shadow: var(--shine-top);
    color: var(--color-text);
    cursor: default;
    animation: player-in var(--motion-base) var(--ease-emphasized) both;
    transition:
      background-color var(--motion-fast) var(--ease-out),
      scale var(--motion-slow) var(--ease-spring);
  }

  .media-disc:hover {
    background: var(--row-pressed);
  }

  .media-disc:focus-visible {
    outline: 2px solid var(--color-accent);
    outline-offset: 1px;
  }

  .media-disc:active {
    scale: 0.92;
    transition-duration: var(--motion-instant);
  }

  .media-ring {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    fill: none;
    stroke: var(--color-fill-strong);
    stroke-width: 1.5;
    pointer-events: none;
  }

  .media-ring-fill {
    stroke: var(--color-text);
    stroke-linecap: round;
  }

  .media-disc-icon,
  .media-disc-glyph {
    display: grid;
    place-items: center;
    grid-area: 1 / 1;
    transition:
      opacity var(--motion-fast) var(--ease-out),
      scale var(--motion-base) var(--ease-spring);
  }

  .media-disc-glyph {
    opacity: 0;
    scale: 0.6;
  }

  .media-disc:is(:hover, :focus-visible) .media-disc-icon {
    opacity: 0;
    scale: 0.6;
  }

  .media-disc:is(:hover, :focus-visible) .media-disc-glyph {
    opacity: 1;
    scale: 1;
  }

  @keyframes player-in {
    from {
      opacity: 0;
      translate: 0 6px;
    }
  }

  @media (forced-colors: active) {
    .media-title,
    .media-artist {
      mask-image: none;
    }
  }
</style>
