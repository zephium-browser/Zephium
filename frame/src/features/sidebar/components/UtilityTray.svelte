<script lang="ts">
  import { tick, type Snippet } from "svelte";
  import { Alert02Icon, Settings05Icon } from "@hugeicons/core-free-icons";
  import Icon from "$shared/ui/Icon";
  import Disclosure from "$shared/ui/Disclosure";

  let {
    label,
    warning = null,
    children,
    onopen,
    onclose,
  }: {
    label: string;
    /** What is wrong with the page in front. The glyph then stays in view as
     *  a warning instead of waiting for the pointer. */
    warning?: string | null;
    /** Told whether the panel is showing. */
    children: Snippet<[boolean]>;
    onopen?: () => void;
    onclose?: () => void;
  } = $props();

  let open = $state(false);
  let body = $state<HTMLElement>();

  // The panel takes the keyboard when it opens, so Escape closes it from
  // wherever it was opened, and after a control in it goes.
  function opened() {
    open = true;
    onopen?.();
    void tick().then(() => body?.focus({ preventScroll: true }));
  }

  function closed() {
    open = false;
    onclose?.();
  }
</script>

<!--
  Everything about the page in front and everything that acts on it, in one
  panel: who it is from, blocking, extensions, what the site may use. One glyph
  holds them, because none of them is worth a permanent seat in the chrome. Each
  section is a direct child of the body, or absent, and the body rules the
  hairline between whichever are showing, except between two that belong
  together (data-tray-tools).
-->
<Disclosure
  {label}
  triggerTitle={warning ?? undefined}
  triggerClass={warning ? "utilities-trigger utilities-warning" : "utilities-trigger"}
  panelClass="utilities-panel sidebar-softens"
  align="end"
  onopen={opened}
  onclose={closed}
>
  {#snippet trigger()}<Icon icon={warning ? Alert02Icon : Settings05Icon} size={15} />{/snippet}
  <div class="tray-body" data-tray-body tabindex="-1" bind:this={body}>
    {@render children(open)}
  </div>
</Disclosure>

<style>
  /*
    Present but unpainted until the column is in use. It acts on the page the
    field names, so it has no business drawing the eye the rest of the time;
    the pointer resting in the column brings it in after a beat, and focusing
    the field or opening its own panel brings it at once. A page that is not
    secure is the exception: that is not something to wait for the pointer to
    learn.
  */
  :global(.browser-sidebar[data-pointer-in="true"]) :global(.utilities-trigger) {
    opacity: 1;
    transition-delay: 120ms, 0ms, 0ms;
  }

  /* Over the field, with its panel or its keyboard, and on the card that
     only comes out when wanted, there is nothing to wait for. */
  :global(.utilities-trigger[data-state="open"]),
  :global(.browser-sidebar[data-pointer-in="true"] .address-field:hover)
    :global(.utilities-trigger),
  :global(.browser-sidebar[data-peek]) :global(.utilities-trigger),
  :global(.address-field:focus-within) :global(.utilities-trigger) {
    opacity: 1;
    transition-delay: 0ms;
  }

  :global(.utilities-trigger) {
    opacity: 0;
    display: grid;
    place-items: center;
    width: 26px;
    height: 26px;
    border: 0;
    border-radius: var(--radius-inset);
    background: transparent;
    color: var(--color-faint);
    cursor: default;
    outline: none;
    transition:
      opacity var(--motion-slow) var(--ease-smooth),
      background-color var(--motion-fast) var(--ease-out),
      color var(--motion-fast) var(--ease-out);
  }

  :global(.utilities-trigger.utilities-warning) {
    opacity: 1;
    color: var(--color-danger);
  }

  :global(.utilities-trigger:hover),
  :global(.utilities-trigger[data-state="open"]) {
    background: var(--row-hover);
    color: var(--color-text);
  }

  :global(.utilities-trigger.utilities-warning:hover),
  :global(.utilities-trigger.utilities-warning[data-state="open"]) {
    color: var(--color-danger);
  }

  :global(.utilities-trigger:focus-visible) {
    box-shadow: var(--shadow-field-focus);
  }

  /* Under the field, as wide as it and from its edge: the trigger sits a
     field-padding in. The rows need the room: a name, its count and a switch
     on one line. */
  :global(.utilities-panel.ui-disclosure-panel) {
    inset-inline-end: -4px;
    inline-size: var(--address-width, min(304px, var(--sidebar-width, 320px)));
    max-width: none;
    max-block-size: calc(100vh - 120px);
    padding: var(--menu-pad);
    overflow-y: auto;
  }

  .tray-body {
    display: flex;
    flex-direction: column;
    outline: none;
  }

  .tray-body > :global(* + *) {
    margin-block-start: 4px;
    padding-block-start: 4px;
    border-block-start: 1px solid var(--color-menu-separator);
  }

  .tray-body > :global([data-tray-tools] + [data-tray-tools]) {
    margin-block-start: 0;
    padding-block-start: 0;
    border-block-start: 0;
  }
</style>
