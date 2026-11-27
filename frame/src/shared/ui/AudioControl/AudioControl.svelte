<script lang="ts">
  import { VolumeHighIcon, VolumeOffIcon } from "@hugeicons/core-free-icons";
  import * as m from "$shared/i18n/messages";
  import Icon from "$shared/ui/Icon";
  type AudioDisplayState = { muted: boolean; can_mute: boolean };

  let {
    audio,
    size,
    offset,
    onToggle,
  }: {
    audio: AudioDisplayState;
    /** The favicon this rides on, in pixels. */
    size: number;
    /** The favicon's inline start inside the host; centred when absent. */
    offset?: number;
    /** Asks for the opposite of the mute shown; native state answers. */
    onToggle: (muted: boolean) => void;
  } = $props();

  let status = $derived(audio.muted ? m.tab_audio_muted() : m.tab_audio_playing());
  let icon = $derived(audio.muted ? VolumeOffIcon : VolumeHighIcon);
</script>

<!--
  Sound is told on the site's mark itself, so the title never moves: a small
  disc in the favicon's corner, and the whole mark becomes the speaker while
  the pointer is on it. The host carries `data-audio-host` so the favicon can
  step aside (styles/favicon.css).
-->
{#if audio.can_mute}
  <button
    type="button"
    class="audio-mark"
    data-audio-mark
    data-muted={audio.muted || undefined}
    data-placed={offset !== undefined || undefined}
    style:--audio-mark={`${size}px`}
    style:--audio-offset={offset === undefined ? undefined : `${offset}px`}
    title={status}
    aria-label={audio.muted ? m.tab_unmute() : m.tab_mute()}
    aria-pressed={audio.muted}
    onclick={(event) => {
      event.stopPropagation();
      onToggle(!audio.muted);
    }}
    onpointerdown={(event) => event.stopPropagation()}
    oncontextmenu={(event) => event.stopPropagation()}
    data-zephium-audio-control
  >
    <span class="audio-glyph"><Icon {icon} size={Math.round(size * 0.9)} /></span>
    <span class="audio-badge"><Icon {icon} size={8} strokeWidth={2.2} /></span>
  </button>
{:else}
  <span
    class="audio-mark"
    data-muted={audio.muted || undefined}
    data-placed={offset !== undefined || undefined}
    style:--audio-mark={`${size}px`}
    style:--audio-offset={offset === undefined ? undefined : `${offset}px`}
    role="img"
    title={status}
    aria-label={status}
    data-zephium-audio-control
  >
    <span class="audio-badge"><Icon {icon} size={8} strokeWidth={2.2} /></span>
  </span>
{/if}

<style>
  .audio-mark {
    --audio-pad: 3px;

    position: absolute;
    inset: 0;
    z-index: 1;
    display: block;
    inline-size: calc(var(--audio-mark) + 2 * var(--audio-pad));
    block-size: calc(var(--audio-mark) + 2 * var(--audio-pad));
    margin: auto;
    padding: 0;
    border: 0;
    border-radius: var(--radius-control-compact);
    background: transparent;
    color: var(--color-label-secondary);
    cursor: default;
    transition: background-color var(--motion-fast) var(--ease-out);
  }

  .audio-mark[data-placed] {
    inset-inline: calc(var(--audio-offset) - var(--audio-pad)) auto;
  }

  span.audio-mark {
    pointer-events: none;
  }

  /* The button alone is the hit area, and it never moves or resizes: what it
     draws shrinks, fades and grows inside it, but cannot take the pointer, or
     the badge scaling away from under the pointer would end the hover that
     scaled it. The badge hangs past the box by its offset; the box reaches
     out to it rather than leaving a strip that is drawn but not pressable. */
  button.audio-mark::after {
    content: "";
    position: absolute;
    inset: 0;
    inset-block-end: -2px;
    inset-inline-end: -2px;
  }

  .audio-glyph,
  .audio-badge {
    position: absolute;
    display: grid;
    place-items: center;
    pointer-events: none;
  }

  .audio-glyph {
    inset: 0;
    opacity: 0;
    scale: 0.6;
    transition:
      opacity var(--motion-fast) var(--ease-out),
      scale var(--motion-base) var(--ease-spring);
  }

  /* The disc sits in a notch the favicon cuts for it, so it reads on any
     ground, light or dark, without a ring of its own. */
  .audio-badge {
    inset-block-end: -2px;
    inset-inline-end: -2px;
    inline-size: 12px;
    block-size: 12px;
    border-radius: var(--radius-capsule);
    background: var(--color-float);
    color: var(--color-text);
    transition:
      opacity var(--motion-fast) var(--ease-out),
      scale var(--motion-base) var(--ease-spring);
  }

  .audio-mark[data-muted] .audio-badge {
    color: var(--color-label-secondary);
  }

  button.audio-mark:is(:hover, :focus-visible) {
    background: var(--row-pressed);
    color: var(--color-text);
  }

  button.audio-mark:is(:hover, :focus-visible) .audio-glyph {
    opacity: 1;
    scale: 1;
  }

  button.audio-mark:is(:hover, :focus-visible) .audio-badge {
    opacity: 0;
    scale: 0.4;
  }

  button.audio-mark:active .audio-glyph {
    scale: 0.88;
    transition-duration: var(--motion-instant);
  }

  button.audio-mark:focus-visible {
    outline: 2px solid var(--color-accent);
    outline-offset: 0;
  }

  @media (forced-colors: active) {
    .audio-badge {
      border: 1px solid CanvasText;
    }
  }
</style>
