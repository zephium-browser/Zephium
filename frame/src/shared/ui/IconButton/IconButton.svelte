<script lang="ts">
  import type { IconSvgElement } from "@hugeicons/svelte";
  import Icon from "../Icon/Icon.svelte";

  let {
    icon,
    label,
    size = 16,
    buttonSize = 28,
    variant = "ghost",
    active = false,
    disabled = false,
    haspopup = false,
    expanded,
    controls,
    class: className = "",
    onclick,
    onmenu,
    pending = false,
    shape = "rounded",
  }: {
    icon: IconSvgElement;
    shape?: "rounded" | "circle";
    variant?: "ghost" | "glass";
    label: string;
    size?: number;
    buttonSize?: number;
    active?: boolean;
    disabled?: boolean;
    pending?: boolean;
    haspopup?: boolean | "menu" | "dialog";
    expanded?: boolean;
    controls?: string;
    class?: string;
    onclick?: (event: MouseEvent) => void;
    /** Opens a menu for the button: on a right click, the context-menu key,
     *  or a press held in place. The click that ends a held press is not a
     *  click. Gets the button's box, for a menu to hang from. */
    onmenu?: (anchor: DOMRect) => void;
  } = $props();

  const HOLD_MS = 450;
  const SLOP = 6;
  let hold: ReturnType<typeof setTimeout> | undefined;
  let held = false;
  let origin = { x: 0, y: 0 };

  function press(event: PointerEvent) {
    // A held press may have opened a menu that swallowed its release, so the
    // next press is what says the last one is over.
    held = false;
    clearTimeout(hold);
    if (!onmenu || event.button !== 0) return;
    origin = { x: event.clientX, y: event.clientY };
    const button = event.currentTarget as HTMLElement;
    hold = setTimeout(() => {
      held = true;
      onmenu(button.getBoundingClientRect());
    }, HOLD_MS);
  }

  function drift(event: PointerEvent) {
    if (Math.hypot(event.clientX - origin.x, event.clientY - origin.y) > SLOP) clearTimeout(hold);
  }

  function release() {
    clearTimeout(hold);
  }

  function menu(event: MouseEvent) {
    if (!onmenu) return;
    event.preventDefault();
    clearTimeout(hold);
    onmenu((event.currentTarget as HTMLElement).getBoundingClientRect());
  }

  function click(event: MouseEvent) {
    if (held) {
      held = false;
      return;
    }
    onclick?.(event);
  }
</script>

<button
  type="button"
  data-shape={shape}
  data-variant={variant}
  aria-label={label}
  title={label}
  aria-pressed={active || undefined}
  aria-haspopup={haspopup === true ? "menu" : haspopup || undefined}
  aria-expanded={haspopup ? expanded : undefined}
  aria-controls={controls}
  disabled={disabled || pending}
  aria-busy={pending || undefined}
  onclick={click}
  onpointerdown={press}
  onpointermove={drift}
  onpointerup={release}
  onpointercancel={release}
  oncontextmenu={menu}
  class={["icon-button", active && "icon-button-active", className]}
  style:--icon-button-size={`${buttonSize}px`}
>
  <Icon {icon} {size} />
</button>

<style>
  /* The raised sibling of the flat chrome button: same footprint, the shared
     control fill on top, and the same plate as every other active thing when
     it is the one that is on. */
  .icon-button[data-variant="glass"] {
    border-radius: var(--radius-control);
    background: var(--color-control);
    color: var(--color-on-control);
    box-shadow: var(--shadow-control);
    transition:
      background-color var(--motion-instant) var(--ease-smooth),
      color var(--motion-instant) var(--ease-smooth),
      box-shadow var(--motion-instant) var(--ease-smooth),
      scale var(--motion-slow) var(--ease-smooth);
  }

  .icon-button[data-variant="glass"][data-shape="circle"] {
    border-radius: var(--radius-capsule);
  }

  .icon-button[data-variant="glass"]:hover:not(:disabled) {
    background: var(--color-control-hover);
    color: var(--color-on-control-strong);
  }

  .icon-button[data-variant="glass"]:active:not(:disabled) {
    background: var(--color-control-pressed);
    transform: none;
    scale: 0.96;
    transition-duration: var(--motion-fast);
  }

  .icon-button[data-variant="glass"].icon-button-active {
    background: var(--row-active);
    box-shadow: var(--row-rim);
    color: var(--color-text);
  }

  @media (forced-colors: active) {
    .icon-button[data-variant="glass"] {
      border: 1px solid ButtonText;
      box-shadow: none;
    }
  }
</style>
