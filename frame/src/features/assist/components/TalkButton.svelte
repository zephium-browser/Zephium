<script lang="ts">
  import * as m from "$shared/i18n/messages";
  import { keymap } from "$domain/keymap";
  import { voiceSession } from "$domain/voice-session";
  import { acceleratorKeys } from "$shared/lib/accelerator";
  import { IS_MAC } from "$shared/platform";

  /**
   * The talk key on screen: hold to talk and let go to send, or press once
   * to keep listening and again to send. It sends intent; whether anything
   * listens is the session's to say.
   */
  const HOLD_MS = 300;

  const view = $derived(voiceSession.view());
  const listening = $derived(view?.state.state === "listening" || view?.state.state === "arming");
  const level = $derived(view?.state.state === "listening" ? voiceSession.inputLevel() : 0);
  const keys = $derived.by(() => {
    const accelerator = keymap.all().find((entry) => entry.id === "voice.talk")?.accelerator;
    return accelerator ? acceleratorKeys(accelerator, IS_MAC).join(IS_MAC ? "" : "+") : null;
  });
  const title = $derived(keys ? m.talk_button_hint({ keys }) : m.talk_button());

  let since: number | null = null;

  function press() {
    if (since !== null) return;
    if (listening) {
      void voiceSession.intent("stop");
      return;
    }
    since = performance.now();
    void voiceSession.intent("start");
  }

  function release() {
    if (since === null) return;
    const held = performance.now() - since >= HOLD_MS;
    since = null;
    if (held) void voiceSession.intent("stop");
  }

  function keydown(event: KeyboardEvent) {
    if (event.key === "Escape" && listening) {
      event.preventDefault();
      event.stopPropagation();
      since = null;
      void voiceSession.intent("cancel");
      return;
    }
    if (event.key !== " " && event.key !== "Enter") return;
    event.preventDefault();
    if (!event.repeat) press();
  }

  function keyup(event: KeyboardEvent) {
    if (event.key !== " " && event.key !== "Enter") return;
    event.preventDefault();
    release();
  }
</script>

<button
  type="button"
  class="talk"
  data-live={listening || undefined}
  aria-label={m.talk_button()}
  aria-pressed={listening}
  {title}
  style:--level={level}
  onpointerdown={(event) => {
    if (event.button !== 0) return;
    press();
    try {
      event.currentTarget.setPointerCapture(event.pointerId);
    } catch {
      // A pointer the platform no longer tracks: the release still arrives.
    }
  }}
  onpointerup={release}
  onpointercancel={release}
  onlostpointercapture={release}
  onkeydown={keydown}
  onkeyup={keyup}
  onblur={release}
>
  <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true">
    <rect class="b1" x="2.25" y="5.5" width="2" height="5" rx="1" />
    <rect class="b2" x="5.5" y="2.5" width="2" height="11" rx="1" />
    <rect class="b3" x="8.75" y="4" width="2" height="8" rx="1" />
    <rect class="b4" x="12" y="6.25" width="2" height="3.5" rx="1" />
  </svg>
</button>

<style>
  .talk {
    --voice-live: var(--color-soft-peach);

    display: grid;
    flex: none;
    place-items: center;
    inline-size: 28px;
    block-size: 28px;
    margin-inline-end: 2px;
    padding: 0;
    border: 0;
    border-radius: 50%;
    background: transparent;
    color: var(--color-muted);
    cursor: default;
    touch-action: none;
    transition:
      background-color var(--motion-fast) var(--ease-out),
      color var(--motion-fast) var(--ease-out),
      scale var(--motion-base) var(--ease-spring);
  }

  .talk:hover {
    background: var(--color-fill-hover);
    color: var(--color-text);
  }

  .talk:active {
    scale: 0.9;
  }

  .talk:focus-visible {
    outline: 2px solid var(--color-ring);
    outline-offset: 2px;
  }

  rect {
    fill: currentcolor;
    transform-box: fill-box;
    transform-origin: 50% 50%;
    transition: transform 90ms linear;
  }

  /* Listening, it is the island's small sibling: the live colour, and its
     bars follow the microphone, redrawn only by level events. */
  .talk[data-live] {
    background: color-mix(in oklab, var(--voice-live) 18%, transparent);
    color: var(--voice-live);
  }

  .talk[data-live] rect {
    transform: scaleY(calc(0.55 + var(--level) * 0.6));
  }

  .talk[data-live] .b2,
  .talk[data-live] .b3 {
    transform: scaleY(calc(0.5 + var(--level) * 0.5));
  }

  @media (prefers-reduced-motion: reduce) {
    rect {
      transition: none;
    }

    .talk[data-live] rect {
      transform: none;
    }
  }

  :global(:root[data-reduce-motion="true"]) .talk[data-live] rect {
    transform: none;
    transition: none;
  }

  @media (forced-colors: active) {
    .talk {
      border: 1px solid ButtonText;
    }
  }
</style>
