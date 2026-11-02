<script lang="ts">
  import type { Snippet } from "svelte";
  import "../Menu/surface.css";
  import { menuMaterialFor } from "../Menu/material";

  let {
    label,
    trigger,
    children,
    menu = false,
    align = "start",
    triggerClass = "",
    panelClass = "",
    onopen,
    onclose,
    triggerTitle,
  }: {
    label: string;
    trigger: Snippet;
    children: Snippet;
    /** Give the panel menu semantics and arrow-key travel over its items. */
    menu?: boolean;
    align?: "start" | "end";
    triggerClass?: string;
    panelClass?: string;
    onopen?: () => void;
    onclose?: () => void;
    /** A native tooltip, the only label that can appear outside a narrow chrome. */
    triggerTitle?: string;
  } = $props();

  let open = $state(false);
  let material = $state<"opaque" | undefined>();
  let root = $state<HTMLElement>();
  let panel = $state<HTMLElement>();
  let button = $state<HTMLButtonElement>();

  const items = () => [
    ...(panel?.querySelectorAll<HTMLElement>(
      '[role="menuitem"]:not(:disabled):not([aria-disabled="true"])',
    ) ?? []),
  ];

  function show(last = false) {
    material = menuMaterialFor(button);
    open = true;
    onopen?.();
    if (!menu) return;
    // A menu opened from the keyboard should already have a target; opened
    // by pointer it should not steal the cursor's place in the list.
    queueMicrotask(() => {
      if (open) (last ? items().at(-1) : items()[0])?.focus();
    });
  }

  function dismiss(restore: boolean) {
    if (!open) return;
    open = false;
    onclose?.();
    if (restore) button?.focus();
  }

  // Closing on the press rather than the click means a press outside never
  // also activates whatever it landed on through a stale open panel.
  $effect(() => {
    if (!open) return;
    const outside = (event: PointerEvent) => {
      if (!(event.target instanceof Node) || !root?.contains(event.target)) dismiss(false);
    };
    // A press on a web page lands in another view and never reaches this
    // document; it takes focus with it, so the window going out of focus
    // is that press, as it is any other application's.
    const away = () => dismiss(false);
    window.addEventListener("pointerdown", outside, true);
    window.addEventListener("blur", away);
    return () => {
      window.removeEventListener("pointerdown", outside, true);
      window.removeEventListener("blur", away);
    };
  });

  function onkeydown(event: KeyboardEvent) {
    if (menu && !open && (event.key === "ArrowDown" || event.key === "ArrowUp")) {
      event.preventDefault();
      show(event.key === "ArrowUp");
      return;
    }
    if (event.key === "Escape" && open) {
      event.stopPropagation();
      dismiss(true);
      return;
    }
    if (!menu || !open) return;
    const list = items();
    const at = list.indexOf(document.activeElement as HTMLElement);
    const step =
      event.key === "ArrowDown"
        ? 1
        : event.key === "ArrowUp"
          ? -1
          : event.key === "Home"
            ? 0
            : null;
    if (event.key === "End") {
      event.preventDefault();
      list.at(-1)?.focus();
    } else if (step === 0) {
      event.preventDefault();
      list[0]?.focus();
    } else if (step !== null && list.length > 0) {
      event.preventDefault();
      list[(at + step + list.length) % list.length]?.focus();
    }
  }
</script>

<!--
  A panel anchored to its own trigger, without a portal and without a
  floating-UI dependency: the chrome loads on every launch, and a two-item
  menu inside a fixed column does not need the machinery a page-wide one
  does. It dismisses on Escape, on a press outside and on losing focus, and
  returns the caret to the trigger when the keyboard closed it.
-->
<div
  bind:this={root}
  class="ui-disclosure"
  role="presentation"
  {onkeydown}
  onfocusout={(event) => {
    // WebKit does not focus a button when it is pressed, so a null
    // relatedTarget means focus went nowhere, not that it left the panel.
    // Treating that as a dismissal closed the menu on the press and the
    // click then landed on nothing. A press outside is handled above.
    const next = event.relatedTarget;
    if (next instanceof Node && !root?.contains(next)) dismiss(false);
  }}
>
  <button
    bind:this={button}
    type="button"
    class={triggerClass}
    aria-label={label}
    title={triggerTitle}
    aria-haspopup={menu ? "menu" : "dialog"}
    aria-expanded={open}
    data-state={open ? "open" : "closed"}
    onclick={() => (open ? dismiss(false) : show())}
  >
    {@render trigger()}
  </button>
  <div
    data-menu-material={material}
    bind:this={panel}
    class={["ui-menu", "ui-disclosure-panel", !menu && "ui-popover", panelClass]}
    role={menu ? "menu" : "group"}
    onclick={(event) => {
      // Choosing from a menu closes it. Owning that here keeps every caller
      // from having to remember, and from needing a handle on this state. A
      // panel's own control that leaves it (opening a page or a dialog) says
      // so with data-dismiss.
      const target = event.target as Element | null;
      if (
        target?.closest("[data-dismiss]") ||
        (menu && !target?.closest("[data-keep-open]") && target?.closest('[role="menuitem"]'))
      )
        dismiss(true);
    }}
    aria-label={label}
    data-align={align}
    data-open={open}
    inert={!open}
  >
    {@render children()}
  </div>
</div>

<style>
  .ui-disclosure {
    position: relative;
    display: flex;
    min-width: 0;
  }
</style>
