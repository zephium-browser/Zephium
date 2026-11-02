<script lang="ts">
  import { onMount, untrack } from "svelte";
  import { Globe02Icon, Search01Icon } from "@hugeicons/core-free-icons";
  import * as m from "$shared/i18n/messages";
  import type { PanelState, ToolKind } from "$shared/ipc/bindings";
  import { commands } from "$shared/ipc/bindings";
  import Icon from "$shared/ui/Icon";
  import FavIcon from "$shared/ui/FavIcon";
  import { favicons } from "$domain/favicons";
  import { IS_MAC, IS_WINDOWS } from "$shared/platform";
  import ResultList from "./ResultList.svelte";
  import LauncherActions, { ACTIONS_ID, type LauncherAction } from "./LauncherActions.svelte";
  import { launcherDestinations } from "../lib/destinations";
  import {
    createSearchSurface,
    type Destination,
    type SurfaceRow,
  } from "../lib/search-surface.svelte";

  let {
    context = null,
    onTool = () => {},
    onDismiss = () => {},
    onCapture,
    hidden = [],
    destinations = launcherDestinations((kind) => !hidden.includes(kind)),
  }: {
    /** The current presentation. The launcher stays mounted while hidden and
     *  binds each new session as it is shown. */
    context?: PanelState | null;
    onTool?: (tool: ToolKind) => void;
    onDismiss?: () => void;
    /** Saves the typed line as a task; resolves to the saved title, or null. */
    onCapture?: (text: string) => Promise<string | null>;
    /** Destinations whose feature is switched off. */
    hidden?: readonly string[];
    destinations?: Destination[];
  } = $props();

  const MOD = IS_MAC ? "⌘" : "Ctrl";
  const keys = {
    open: ["↵"],
    background: [MOD, "↵"],
    copy: IS_MAC ? ["⇧", "⌘", "C"] : ["Ctrl", "Shift", "C"],
    capture: IS_MAC ? ["⌥", "↵"] : ["Alt", "↵"],
  };

  const surface = createSearchSurface({
    tabId: null,
    destinations: untrack(() => destinations),
    onTool: (tool) => onTool(tool),
    onCapture: untrack(() => onCapture),
    captureKeys: keys.capture,
  });

  // Geometry the native shapes are placed from. Kept as numbers rather than
  // read back from the DOM, so a report never depends on a frame mid-motion.
  const FIELD = 60;
  const GAP = 10;
  /** The shapes' width. Native widens the window around it to leave room for
   *  their shadow, so the margin is whatever it left. */
  const WIDTH = 680;
  const FOOTER = 52;
  /** Mirrors the native ceiling on the launcher window: this, or the work
   *  area less a margin above and below on a display too short for it. */
  const WINDOW_MAX = 600;
  const WINDOW_MARGIN = 24;

  let input = $state<HTMLInputElement>();
  let list = $state<HTMLElement>();
  let content = $state<HTMLElement>();
  let contentHeight = $state(0);
  let windowWidth = $state(0);
  let windowHeight = $state(0);
  /** The display's usable height. The window is placed before it moves there,
   *  so this is re-read whenever the window is resized or presented. */
  let workHeight = $state(screen.availHeight);
  let menu = $state(false);
  let menuHeight = $state(0);
  let menuItem = $state<string>();
  let commandHeld = $state(false);
  /** The launcher is arriving; its content settles in with the glass. */
  let entering = $state(false);
  let arrival: ReturnType<typeof setTimeout> | undefined;
  /** Matches the native arrival, which is the longest thing moving. */
  const ARRIVAL_MS = 460;
  let material = $state(document.documentElement.dataset.material ?? "none");

  // Native draws glass or vibrancy as two shapes behind the content. Anything
  // else, including reduced transparency, is one card the page draws itself.
  let detached = $derived(material === "liquid_glass" || material === "vibrancy");
  let inset = $derived(detached ? Math.max(0, (windowWidth - WIDTH) / 2) : 0);
  let gap = $derived(detached ? GAP : 0);
  let nativeClip = $derived(IS_WINDOWS && !detached);
  let ceiling = $derived(Math.max(1, Math.min(WINDOW_MAX, workHeight - 2 * WINDOW_MARGIN)));
  let sheetMax = $derived(
    Math.max(0, (nativeClip ? windowHeight : ceiling) - 2 * inset - FIELD - gap),
  );
  let sheetHeight = $derived(
    Math.min(sheetMax, Math.max(contentHeight + FOOTER, menu ? menuHeight + FOOTER + 8 : 0)),
  );

  let home = $derived(!surface.query.trim());
  let announcement = $derived(
    surface.empty
      ? m.panel_no_results()
      : !home && !surface.pending && surface.rows.length
        ? m.launcher_results_count({ count: surface.rows.length })
        : "",
  );
  let row = $derived(surface.selectedRow);
  let primary = $derived(primaryLabel(row));
  let lead = $derived.by(() => {
    const completed = surface.completedRow?.result;
    const image = completed ? favicons.image(completed.icon) : null;
    if (image) return { image, tone: favicons.tone(completed?.icon) };
    return { icon: surface.rows[0]?.result?.kind === "url" ? Globe02Icon : Search01Icon };
  });
  let actions = $derived.by<LauncherAction[]>(() => {
    const available: LauncherAction[] = [];
    if (row)
      available.push({ id: "open", label: primary, keys: keys.open, run: () => surface.submit() });
    if (surface.addressable) {
      available.push({
        id: "background",
        label: m.launcher_open_background(),
        keys: keys.background,
        run: () => surface.submit(true),
      });
      available.push({
        id: "copy",
        label: m.launcher_copy_link(),
        keys: keys.copy,
        run: () => void surface.copy(),
      });
    }
    if (surface.capturable && row?.id !== "capture:task")
      available.push({
        id: "capture",
        label: m.launcher_add_task(),
        keys: keys.capture,
        run: () => void surface.capture(),
      });
    return available;
  });

  function primaryLabel(target: SurfaceRow | null) {
    if (!target) return m.launcher_open();
    if (target.id === "capture:task") return m.launcher_add_task();
    if (target.calculation) return m.launcher_copy_answer();
    switch (target.result?.kind) {
      case "tab":
        return m.launcher_switch_tab();
      case "search":
      case "suggestion":
      case "search_history":
        return m.launcher_search();
      case "note":
        return m.launcher_open_note();
      case "command":
        return m.launcher_run();
      default:
        return m.launcher_open();
    }
  }

  function failure() {
    if (surface.error === "too_long") return m.panel_query_too_long();
    if (context?.error || surface.failed) return m.panel_action_failed();
    if (surface.error === "failed") return m.launcher_search_failed();
    return null;
  }

  // One session per presentation. Binding happens while the window is being
  // ordered in, so the field is focused and the list is already the right one
  // on the first frame the user sees.
  let bound: string | null = null;
  $effect(() => {
    const next = context;
    untrack(() => {
      if (next?.visible) {
        if (bound === next.session_id) return;
        bound = next.session_id;
        menu = false;
        workHeight = screen.availHeight;
        entering = !IS_WINDOWS;
        clearTimeout(arrival);
        if (entering) arrival = setTimeout(() => (entering = false), ARRIVAL_MS);
        // Editing a page's address opens on that address, selected, so typing
        // replaces it and Return goes back to it.
        surface.begin(next);
        input?.focus();
        input?.select();
      } else if (bound !== null) {
        bound = null;
        menu = false;
        commandHeld = false;
        surface.end();
      }
    });
  });

  $effect(() => {
    surface.setList(list);
  });
  $effect(() => {
    void surface.query;
    surface.applyCompletion(input);
  });
  $effect(() => {
    const element = content;
    if (!element) return;
    const observer = new ResizeObserver(([entry]) => {
      contentHeight = entry!.borderBoxSize[0]!.blockSize;
    });
    observer.observe(element);
    return () => observer.disconnect();
  });

  // Native sizes the window and places its shapes from this. The field sits
  // `inset` from the top, the sheet `gap` below it; both run the full width
  // between the insets.
  let reported = "";
  $effect(() => {
    const width = windowWidth - 2 * inset;
    if (width <= 0) return;
    const top = inset + FIELD + gap;
    const layout = {
      height: Math.ceil(top + sheetHeight + inset),
      field: { x: inset, y: inset, width, height: FIELD },
      sheet: { x: inset, y: top, width, height: Math.ceil(sheetHeight) },
    };
    const key = JSON.stringify(layout);
    if (key === reported) return;
    const send = () => {
      reported = key;
      void commands.panelLayout(layout).catch(() => {
        if (reported === key) reported = "";
      });
    };
    // Hidden WebViews may stop animation frames. Store the next opening size
    // now, without waking the renderer or waiting on a suspended paint loop.
    if (!context?.visible) {
      send();
      return;
    }
    // Let Chromium paint the target rows before native uncovers more of the
    // fixed viewport. Coalesce replacements; hidden/suspended hosts run no loop.
    let paint = 0;
    const frame = requestAnimationFrame(() => {
      paint = requestAnimationFrame(send);
    });
    return () => {
      cancelAnimationFrame(frame);
      cancelAnimationFrame(paint);
    };
  });

  function runAction(action: LauncherAction) {
    menu = false;
    action.run();
  }

  function keydown(event: KeyboardEvent) {
    commandHeld = IS_MAC ? event.metaKey : event.ctrlKey;
    // WebKit ends a composition before the keydown that committed it, with
    // isComposing already false; that key still reports the IME's 229.
    if (event.defaultPrevented || surface.composing || event.isComposing || event.keyCode === 229)
      return;
    const mod = IS_MAC ? event.metaKey : event.ctrlKey;
    const key = event.key.toLowerCase();
    if (mod && key === "k") {
      event.preventDefault();
      menu = !menu && actions.length > 0;
      return;
    }
    const menuOpen = menu;
    if (menu) {
      // The sheet owns navigation while it is open; anything else puts it
      // away and falls through to the field.
      if (event.key === "Escape") {
        event.preventDefault();
        menu = false;
        return;
      }
      if (event.key === "ArrowDown" || event.key === "ArrowUp" || event.key === "Enter") return;
      menu = false;
    }
    // Like Spotlight, Tab does not take the keyboard out of the field.
    if (event.key === "Tab" && !menuOpen && event.target === input) {
      event.preventDefault();
      return;
    }
    // A focused button activates itself instead of running the field's row.
    if (event.key === "Enter" && event.target instanceof HTMLButtonElement) return;
    const numbered = mod && !event.shiftKey && !event.altKey ? Number(key) : NaN;
    if (numbered >= 1 && numbered <= destinations.length) {
      event.preventDefault();
      onTool(destinations[numbered - 1]!.kind);
      return;
    }
    const down = event.key === "ArrowDown" || (IS_MAC && event.ctrlKey && key === "n");
    const up = event.key === "ArrowUp" || (IS_MAC && event.ctrlKey && key === "p");
    if (down || up) {
      event.preventDefault();
      void surface.move(down ? 1 : -1);
    } else if (event.key === "Enter" && event.altKey) {
      event.preventDefault();
      void surface.capture();
    } else if (event.key === "Enter") {
      event.preventDefault();
      surface.submit(mod);
    } else if (mod && event.shiftKey && key === "c") {
      event.preventDefault();
      void surface.copy();
    } else if (event.key === "Escape") {
      event.preventDefault();
      // Spotlight's order: the first press clears what was typed, the second
      // puts the launcher away.
      if (surface.query) {
        surface.changed("");
        input?.focus();
      } else onDismiss();
    }
  }

  onMount(() => {
    const refocus = () => input?.focus();
    window.addEventListener("focus", refocus);
    const materials = new MutationObserver(() => {
      material = document.documentElement.dataset.material ?? "none";
    });
    materials.observe(document.documentElement, { attributeFilter: ["data-material"] });
    const unmount = surface.mount();
    return () => {
      clearTimeout(arrival);
      window.removeEventListener("focus", refocus);
      materials.disconnect();
      unmount();
    };
  });
</script>

<svelte:window
  bind:innerWidth={windowWidth}
  bind:innerHeight={windowHeight}
  onresize={() => (workHeight = screen.availHeight)}
  onkeydown={keydown}
  onkeyup={(event) => (commandHeld = IS_MAC ? event.metaKey : event.ctrlKey)}
/>

<!-- The margin around the shapes is empty window. A click there is a click
     outside the launcher, and puts it away as one would. -->
<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  class="launcher"
  class:entering
  onpointerdown={(event) => {
    if (event.target === event.currentTarget) onDismiss();
    // A press anywhere but the field itself leaves the caret in it.
    else if (event.pointerType !== "touch" && !(event.target as Element).closest("input"))
      event.preventDefault();
  }}
  data-layout={detached ? "detached" : "fused"}
  data-native-clip={nativeClip}
  style:--inset={`${inset}px`}
>
  <div class="capsule">
    <span class="lead" aria-hidden="true"
      >{#if lead.image}<FavIcon image={lead.image} tone={lead.tone} size={16} lit />{:else}<Icon
          icon={lead.icon}
          size={20}
        />{/if}</span
    ><input
      bind:this={input}
      value={surface.query}
      oninput={(event) => surface.changed(event.currentTarget.value)}
      oncompositionstart={surface.compositionStart}
      oncompositionend={(event) => surface.compositionEnd(event.currentTarget.value)}
      maxlength={2048}
      autocomplete="off"
      autocapitalize="off"
      autocorrect="off"
      spellcheck={false}
      placeholder={m.panel_search_placeholder()}
      aria-label={m.panel_search_placeholder()}
      role="combobox"
      aria-haspopup={menu ? "menu" : "listbox"}
      aria-controls={menu ? ACTIONS_ID : "launcher-results"}
      aria-expanded={menu || surface.rows.length > 0}
      aria-autocomplete="both"
      aria-activedescendant={menu
        ? menuItem
        : surface.selectedIndex >= 0
          ? `launcher-results-option-${surface.selectedIndex}`
          : undefined}
    />
  </div>
  <!-- Rows arrive without the focus moving to them, so a screen reader hears
       what came back from here. Quiet at home, where nothing was asked. -->
  <div class="sr-only" role="status">{announcement}</div>

  <div class="sheet" style:height={`${sheetHeight}px`} style:margin-top={`${gap}px`}>
    <div class="scroll" style:max-height={`${Math.max(0, sheetMax - FOOTER)}px`}>
      <div class="content" bind:this={content}>
        {#if failure()}
          <div class="failure" role="alert">
            <span>{failure()}</span>
            <button type="button" onclick={surface.retry}>{m.panel_retry()}</button>
          </div>
        {/if}
        <ResultList
          rows={surface.rows}
          selected={surface.selectedId}
          query={surface.answered}
          listId="launcher-results"
          busy={surface.pending}
          variant="launcher"
          bind:ref={list}
          onhover={surface.hover}
          onrun={surface.activate}
        />
        {#if surface.empty}<p class="empty">{m.panel_no_results()}</p>{/if}
      </div>
    </div>

    <footer>
      <div class="start" aria-live="polite">
        {#if surface.captured}<span class="status"
            >{m.launcher_captured({ title: surface.captured })}</span
          >{:else if surface.notice === "opened"}<span class="status">{m.launcher_opened()}</span
          >{:else if surface.notice === "copied"}<span class="status">{m.launcher_copied()}</span
          >{:else if surface.notice === "answer"}<span class="status"
            >{m.launcher_answer_copied()}</span
          >{:else if surface.running}<span class="status">{m.panel_opening()}</span>{:else if home}
          <div class="destinations" role="group" aria-label={m.launcher_destinations()}>
            {#each destinations as destination (destination.kind)}
              <button
                type="button"
                class="chip"
                aria-keyshortcuts={destination.keys?.join("+")}
                onpointerdown={(event) => event.preventDefault()}
                onclick={() => onTool(destination.kind)}
                ><Icon icon={destination.icon} size={15} /><span>{destination.label}</span
                >{#if commandHeld && destination.keys}<kbd>{destination.keys.at(-1)}</kbd
                  >{/if}</button
              >
            {/each}
          </div>
        {/if}
      </div>
      {#if row}<div class="actions">
          <span class="primary">{primary}<kbd>↵</kbd></span>
          {#if actions.length > 1}<span class="rule" aria-hidden="true"></span><button
              type="button"
              class="more"
              aria-label={m.launcher_actions()}
              aria-expanded={menu}
              aria-haspopup="menu"
              onpointerdown={(event) => event.preventDefault()}
              onclick={() => (menu = !menu)}
              >{m.launcher_actions()}<kbd>{MOD}</kbd><kbd>K</kbd></button
            >{/if}
        </div>{/if}
    </footer>

    {#if menu}<LauncherActions
        {actions}
        bind:height={menuHeight}
        onhighlight={(id) => (menuItem = id)}
        onrun={runAction}
        onclose={() => (menu = false)}
      />{/if}
  </div>
</div>

<style>
  /* Rows sit six points inside the glass sheet, so their corner is its 24
     less six. Inside a drawn card they follow the card's own corner, which
     on Windows lands on the system's 4-point list radius. */
  .launcher {
    --row-radius: var(--radius-card);

    display: flex;
    flex-direction: column;
    box-sizing: border-box;
    padding: var(--inset);
    /* stylelint-disable-next-line property-no-vendor-prefix */
    -webkit-user-select: none;
    user-select: none;
  }

  /* Without native shapes the page draws the launcher as one card, and the
     window supplies its shadow. On Windows the system also draws the rim at
     its own radius, which the card takes from the host. */
  .launcher[data-layout="fused"] {
    --row-radius: calc(var(--panel-radius, var(--radius-panel)) - 4px);

    height: fit-content;
    overflow: hidden;
    border-radius: var(--panel-radius, var(--radius-panel));
    background: color-mix(in srgb, var(--color-chrome) var(--wash-launcher), transparent);
    box-shadow: inset 0 0 0 1px var(--color-border-strong);
  }

  :global(:root:is([data-material="acrylic"], [data-material="mica"]))
    .launcher[data-layout="fused"] {
    box-shadow: none;
  }

  /* One wash covers the stable Windows renderer surface. The HWND alone
     owns its visible bottom edge and rounded rim, so a resize cannot reveal
     an untinted strip between independently sized CSS and native cards. */
  .launcher[data-native-clip="true"] {
    height: 100vh;
    border-radius: 0;
    box-shadow: none;
  }

  .capsule {
    display: flex;
    align-items: center;
    gap: 14px;
    flex: none;
    height: 60px;
    padding: 0 24px 0 22px;
    color: var(--color-muted);
  }

  .lead {
    display: grid;
    place-items: center;
    flex: none;
    width: 22px;
    height: 22px;
  }

  input {
    min-width: 0;
    flex: 1;
    height: 100%;
    border: 0;
    background: transparent;
    color: var(--color-text);
    font-size: 21px;
    letter-spacing: -0.016em;
    outline: none;
    caret-color: var(--color-text);
    user-select: text;
  }

  input::placeholder {
    color: var(--color-faint);
    opacity: 1;
  }

  input::selection {
    background: var(--color-fill-strong);
  }

  .sheet {
    position: relative;
    display: flex;
    flex-direction: column;
    flex: none;
    overflow: hidden;
  }

  /* Native moves the glass over this same interval and curve, so the content
     and its material settle together. */
  .launcher[data-layout="detached"] .sheet {
    transition: height 260ms cubic-bezier(0.2, 0.8, 0.2, 1);
  }

  .launcher[data-layout="fused"] .sheet {
    border-top: 1px solid var(--color-border);
  }

  .scroll {
    flex: 1 1 auto;
    min-height: 0;
    overflow-y: auto;
    overscroll-behavior: contain;
    scrollbar-width: none;
  }

  .scroll::-webkit-scrollbar {
    display: none;
  }

  /* Six points in from a 24-point sheet, so a row's 18-point corner is
     concentric with the glass around it. */
  .content {
    padding: 6px 6px 0;
  }

  footer {
    display: flex;
    align-items: center;
    gap: 12px;
    flex: none;
    height: 52px;
    padding: 0 10px 0 12px;
    font-size: 12.5px;
    color: var(--color-muted);
  }

  .start {
    flex: 1;
    min-width: 0;
  }

  .status {
    display: block;
    padding-inline-start: 6px;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .destinations {
    display: flex;
    gap: 4px;
  }

  .chip {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    height: 30px;
    padding: 0 10px 0 9px;
    border: 0;
    border-radius: var(--radius-capsule);
    background: transparent;
    color: var(--color-muted);
    font-size: 12.5px;
    transition:
      background-color var(--motion-instant) var(--ease-smooth),
      color var(--motion-instant) var(--ease-smooth);
  }

  .chip:hover {
    background: var(--color-fill-hover);
    color: var(--color-text);
  }

  .actions {
    display: flex;
    align-items: center;
    gap: 2px;
    flex: none;
    height: 34px;
    padding: 0 4px 0 14px;
    border-radius: var(--radius-capsule);

    /* A raised capsule on the sheet, lit along its edges like the glass. */
    background: var(--color-tile);
    box-shadow: var(--shine), var(--shadow-tile);
    color: var(--color-text);
    font-size: 12.5px;
    font-weight: 500;
  }

  .primary,
  .more {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    height: 26px;
  }

  .primary {
    padding-inline-end: 6px;
  }

  .more {
    padding: 0 6px 0 8px;
    border: 0;
    border-radius: var(--radius-capsule);
    background: transparent;
    color: var(--color-muted);
    font-weight: 500;
    transition: background-color var(--motion-instant) var(--ease-smooth);
  }

  .more:hover,
  .more[aria-expanded="true"] {
    background: var(--color-fill-hover);
    color: var(--color-text);
  }

  .rule {
    width: 1px;
    height: 14px;
    background: var(--color-border-strong);
  }

  kbd {
    display: inline-grid;
    place-items: center;
    box-sizing: border-box;
    min-width: 18px;
    height: 18px;
    padding: 0 4px;
    border-radius: 5px;

    /* Drawn keycaps: an outline the key sits in, never a filled chip. */
    background: transparent;
    box-shadow: inset 0 0 0 1px var(--color-border-strong);
    color: var(--color-muted);
    font-family: var(--font-sans);
    font-size: 11px;
    font-weight: 500;
  }

  .primary > kbd:first-of-type,
  .more > kbd:first-of-type {
    margin-inline-start: 4px;
  }

  .empty {
    margin: 0;
    padding: 18px 14px 10px;
    font-size: 13.5px;
    color: var(--color-faint);
  }

  .failure {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 6px 6px 6px 14px;
    font-size: 13px;
    color: var(--color-muted);
  }

  .failure button {
    height: 28px;
    padding: 0 12px;
    border: 0;
    border-radius: var(--radius-capsule);
    background: var(--color-fill);
    color: var(--color-text);
  }

  /* Arrival. The native glass grows the sheet out of the capsule; the content
     is revealed from the top at the same pace and its rows drift down into
     place a moment behind, so the list reads as poured rather than switched
     on. Where the page draws the card itself it simply scales in. */
  .launcher.entering .capsule > * {
    animation: arrive-fade 200ms var(--ease-out) both;
  }

  .launcher.entering[data-layout="detached"] .sheet {
    animation: arrive-reveal 420ms cubic-bezier(0.22, 1, 0.36, 1) both;
  }

  .launcher.entering .scroll,
  .launcher.entering footer {
    animation: arrive-drift 360ms var(--ease-out) 60ms both;
  }

  .launcher.entering[data-layout="fused"] {
    animation: arrive-scale 220ms var(--ease-out) both;
  }

  @keyframes arrive-fade {
    from {
      opacity: 0;
    }
  }

  @keyframes arrive-reveal {
    from {
      clip-path: inset(0 0 100% 0 round 24px);
    }

    to {
      clip-path: inset(0 0 0 0 round 24px);
    }
  }

  @keyframes arrive-drift {
    from {
      opacity: 0;
      transform: translateY(-8px);
    }
  }

  @keyframes arrive-scale {
    from {
      opacity: 0;
      transform: scale(0.97);
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .launcher[data-layout="detached"] .sheet,
    .chip,
    .more {
      transition: none;
    }
  }

  @media (forced-colors: active) {
    .launcher[data-layout="fused"] {
      border: 1px solid CanvasText;
    }

    .actions {
      border: 1px solid CanvasText;
    }
  }
</style>
