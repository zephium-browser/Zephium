<script lang="ts">
  import { onMount } from "svelte";
  import * as m from "$shared/i18n/messages";
  import { commands, type LauncherTrigger } from "$shared/ipc/bindings";
  import { events } from "$shared/ipc/native-events";
  import { acceleratorKeys } from "$shared/lib/accelerator";
  import { IS_MAC } from "$shared/platform";
  import { heldFrom, heldParts, NOTHING_HELD, type Held } from "../../lib/keys";
  import LauncherScene from "../LauncherScene.svelte";

  let trigger = $state<LauncherTrigger | null>(null);
  let location = $state<string | null>(null);
  let held = $state<Held>(NOTHING_HELD);
  /** The drawn launcher has opened and is typing. */
  let typing = $state(false);
  /** The real launcher is up, answering the real shortcut. */
  let real = $state(false);
  let keys = $derived(trigger ? acceleratorKeys(trigger.shortcut, IS_MAC) : []);
  let down = $derived(trigger ? heldParts(trigger.shortcut, held, IS_MAC) : []);
  let locationKeys = $derived(location ? acceleratorKeys(location, IS_MAC) : []);
  let locationDown = $derived(location ? heldParts(location, held, IS_MAC) : []);

  onMount(() => {
    let live = true;
    void commands.launcherTrigger().then(
      (value) => {
        if (live) trigger = value;
      },
      () => {},
    );
    void commands.keymapEntries().then(
      (entries) => {
        if (live) location = entries.find((entry) => entry.id === "url.focus")?.accelerator ?? null;
      },
      () => {},
    );
    // Opens on its own once the scene has risen, so the launcher is the
    // first thing seen here.
    const opening = setTimeout(() => (typing = true), 900);
    const listening = events.uiCommand.listen(({ payload }) => {
      if (payload === "launcher.presented") real = true;
      if (payload === "launcher.dismissed") real = false;
    });
    return () => {
      live = false;
      clearTimeout(opening);
      void listening.then((stop) => stop());
    };
  });

  const track = (event: KeyboardEvent) => (held = heldFrom(event, held));
  // Keys let go while the launcher has focus never reach this window.
  const release = () => (held = NOTHING_HELD);
</script>

<svelte:window onkeydown={track} onkeyup={track} onblur={release} />

<div class="launcher">
  <LauncherScene {typing} away={real} />
  <div class="try">
    <div class="shortcut">
      <div class="keys" aria-label={keys.join(" ")}>
        {#each keys as key, index (index)}
          <kbd data-down={down[index] ?? false} data-lit={real} data-wide={key.length > 2}
            >{key}</kbd
          >
        {/each}
      </div>
      <p>
        {trigger && !trigger.registered ? m.launcher_shortcut_unregistered() : m.onb_launcher_try()}
      </p>
    </div>
    {#if location}
      <div class="shortcut">
        <div class="keys" aria-label={locationKeys.join(" ")}>
          {#each locationKeys as key, index (index)}
            <kbd data-down={locationDown[index] ?? false} data-lit={real} data-wide={key.length > 2}
              >{key}</kbd
            >
          {/each}
        </div>
        <p>{m.onb_launcher_browser_try()}</p>
      </div>
    {/if}
  </div>
</div>

<style>
  .launcher {
    display: grid;
    justify-items: center;
    gap: 34px;
  }

  .try {
    display: grid;
    justify-items: center;
    gap: 22px;
    animation: lift 640ms var(--ease-emphasized) 320ms backwards;
  }

  .shortcut {
    display: grid;
    justify-items: center;
    gap: 12px;
  }

  .keys {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  kbd {
    display: grid;
    place-items: center;
    min-inline-size: 38px;
    block-size: 36px;
    padding: 0 10px;
    box-sizing: border-box;
    border-radius: var(--radius-control);
    background: var(--color-control);
    box-shadow: var(--shadow-raise);
    color: var(--color-text);
    font-family: var(--font-sans);
    font-size: 15px;
    font-weight: 500;
    transition:
      transform var(--motion-fast) var(--ease-press),
      background-color var(--motion-base) var(--ease-out),
      color var(--motion-base) var(--ease-out);
  }

  kbd[data-wide="true"] {
    min-inline-size: 78px;
    font-size: 13px;
  }

  kbd[data-down="true"] {
    background: var(--color-control-pressed);
    transform: translateY(1px) scale(0.97);
  }

  kbd[data-lit="true"] {
    background: var(--color-lit);
    color: var(--color-on-lit);
  }

  p {
    margin: 0;
    color: var(--color-faint);
    font-size: 12.5px;
  }

  @keyframes lift {
    from {
      opacity: 0;
      transform: translateY(8px);
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .try {
      animation: none;
    }
  }
</style>
