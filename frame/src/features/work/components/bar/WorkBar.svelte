<script lang="ts">
  import { untrack, type Snippet } from "svelte";
  import Icon from "$shared/ui/Icon";
  import { keymap } from "$domain/keymap";
  import { uiCommands } from "$domain/ui-commands";
  import { acceleratorKeys } from "$shared/lib/accelerator";
  import { IS_MAC } from "$shared/platform";
  import { ArrowUp02Icon, Mic01Icon } from "../../lib/icons";
  import * as m from "$shared/i18n/messages";

  const loadVoice = () => import("./VoiceView.svelte");

  let {
    value = $bindable(""),
    placeholder,
    disabled = false,
    busy = false,
    holding = false,
    model = null,
    tools,
    attach,
    context,
    above,
    trailing,
    onsubmit,
    ref = $bindable(),
    surface = $bindable(),
  }: {
    value?: string;
    placeholder: string;
    disabled?: boolean;
    busy?: boolean;
    /** Something the field opened is still in use, so it stays open. */
    holding?: boolean;
    /** The model the runs here use, when a run has said. */
    model?: string | null;
    tools?: Snippet;
    attach?: Snippet;
    context?: Snippet;
    above?: Snippet;
    /** What stands before the mic, such as the model the runs use; it replaces the model's name. */
    trailing?: Snippet;
    onsubmit: () => void;
    ref?: HTMLElement;
    /** The sheet the words sit on; the canvas cuts its window-through hole to its shape. */
    surface?: HTMLElement;
  } = $props();
  const id = $props.id();
  let textarea = $state<HTMLTextAreaElement>();
  let focused = $state(false);
  /** A menu or popover the field's own controls opened: it may live in a portal, but it is the bar's. */
  let menu = $state(false);
  const opened = (element: Element | undefined) =>
    !!element?.querySelector('.field [aria-expanded="true"]');
  $effect(() => {
    const element = surface;
    if (!element) return;
    const observer = new MutationObserver(() => {
      const open = opened(element);
      if (open === menu) return;
      menu = open;
      // WebKit can leave focus on body while a portal closes. Only an
      // actual outside press/focus transfer releases the composer's focus.
    });
    observer.observe(element, { subtree: true, attributeFilter: ["aria-expanded"] });
    return () => observer.disconnect();
  });
  function owns(target: Node | null): boolean {
    if (!target || !surface) return false;
    if (surface.contains(target)) return true;
    // Bits UI's non-modal popover content need not expose aria-controls.
    // While this field's trigger is expanded, its active portal is part of the field.
    if (
      (menu || opened(surface)) &&
      target instanceof Element &&
      target.closest("[data-popover-content], [data-dropdown-menu-content]")
    )
      return true;
    return [...surface.querySelectorAll(".field [aria-controls]")].some((trigger) => {
      const controlled = document.getElementById(trigger.getAttribute("aria-controls") ?? "");
      return controlled?.contains(target) ?? false;
    });
  }
  /** Dictating: the row is the live voice view until the words land. */
  let dictating = $state(false);
  let voice = $state<{ toggle(): void }>();
  /** There are words to send: the mic gives way to Send. */
  const writing = $derived(!!value.trim());
  const engaged = $derived(focused || menu || writing || holding || dictating);
  /**
   * WebKit does not focus a button it clicks: a press inside the bar blurs
   * the field with nowhere named to go. The bar holds through its own
   * presses and lets go on a press outside it (a menu it opened aside).
   */
  let pressing = false;
  const mode = $derived(engaged ? "focus" : "rest");

  function grow() {
    const element = textarea;
    if (!element) return;
    element.style.blockSize = "auto";
    element.style.blockSize = `${Math.min(element.scrollHeight, 132)}px`;
  }
  $effect(() => {
    void value;
    grow();
  });
  /** Blank lines are not a message: the bar lets go of them with the focus and rests at its own height. */
  $effect(() => {
    if (!engaged && value && !value.trim()) value = "";
  });
  function submit() {
    if (writing && !disabled && !dictating) onsubmit();
  }

  function dictate() {
    if (dictating) voice?.toggle();
    else if (!disabled) dictating = true;
  }

  /** The words join what is already there, ready to read over and send. */
  function commit(text: string) {
    const before = value.trimEnd();
    value = before ? `${before} ${text}` : text;
    dictating = false;
    requestAnimationFrame(() => {
      const element = textarea;
      if (!element) return;
      element.focus();
      element.setSelectionRange(element.value.length, element.value.length);
    });
  }

  let handledCommand = untrack(() => uiCommands.uiCommand().seq);
  $effect(() => {
    const command = uiCommands.uiCommand();
    if (command.seq === handledCommand) return;
    handledCommand = command.seq;
    if (command.id === "work.dictate") untrack(dictate);
  });

  const dictateHint = $derived.by(() => {
    const accelerator = keymap.all().find((entry) => entry.id === "work.dictate")?.accelerator;
    const keys = accelerator ? acceleratorKeys(accelerator, IS_MAC) : [];
    return keys.length ? `${m.voice_start()} (${keys.join(IS_MAC ? "" : "+")})` : m.voice_start();
  });
  function keydown(event: KeyboardEvent) {
    if (event.key === "Enter" && !event.shiftKey && !event.isComposing) {
      event.preventDefault();
      submit();
    }
    if (event.key === "Escape" && !writing) textarea?.blur();
  }
</script>

<svelte:window
  onpointerdown={(event) => {
    const target = event.target instanceof Node ? event.target : null;
    if (focused && !owns(target)) focused = false;
  }}
  onfocusin={(event) => {
    const target = event.target instanceof Node ? event.target : null;
    if (focused && !owns(target)) focused = false;
  }}
  onpointerup={() => requestAnimationFrame(() => (pressing = false))}
  onpointercancel={() => (pressing = false)}
/>

<div class="work-bar" data-mode={mode} data-engaged={engaged} bind:this={ref}>
  {#if above}<div class="above">{@render above()}</div>{/if}
  <div
    class="surface"
    role="presentation"
    bind:this={surface}
    onpointerdown={(event) => {
      if (event.target instanceof Element && event.target.closest(".field")) pressing = true;
    }}
    onfocusin={(event) => {
      // A tool's own panel is the tools at work, not the field: the bar stays as it is.
      if (event.target instanceof Element && event.target.closest(".tools")) return;
      focused = true;
    }}
    onfocusout={(event) => {
      const next = event.relatedTarget;
      if (next instanceof Node && event.currentTarget.contains(next)) return;
      if (pressing && !next) return;
      // A menu the field's own control opened holds the bar open while it is open, portals included.
      if (menu || opened(event.currentTarget)) return;
      focused = false;
    }}
  >
    {#if tools}
      <div class="tools" inert={mode !== "rest"} aria-hidden={mode !== "rest"}>
        <div class="tools-inner" role="toolbar" aria-label={m.work_env_toolbar()}>
          {@render tools()}
          <span class="rule" aria-hidden="true"></span>
        </div>
      </div>
    {/if}
    <form
      class="field"
      onsubmit={(event) => {
        event.preventDefault();
        submit();
      }}
    >
      {#if dictating}<div class="row">
          {#await loadVoice() then view}<view.default
              bind:this={voice}
              oncommit={commit}
              onclose={() => (dictating = false)}
            />{/await}
        </div>{/if}
      <div class="row" hidden={dictating}>
        {#if attach}<div class="side lead" inert={!engaged}>
            <div class="side-inner">{@render attach()}</div>
          </div>{/if}
        {#if context && engaged}<div class="context">{@render context()}</div>{/if}
        <label class="sr-only" for={id}>{placeholder}</label>
        <textarea
          {id}
          bind:this={textarea}
          bind:value
          rows="1"
          {placeholder}
          maxlength="8192"
          {disabled}
          onkeydown={keydown}
          oninput={grow}></textarea>
        <div class="trail">
          {#if trailing && engaged}{@render trailing()}
          {:else if model && engaged}<span class="model" title={m.work_bar_model()}>{model}</span
            >{/if}
          <div class="action" data-send={writing}>
            <button
              type="button"
              class="mic"
              aria-label={m.voice_start()}
              title={dictateHint}
              {disabled}
              inert={writing}
              onclick={dictate}
            >
              <Icon icon={Mic01Icon} size={17} strokeWidth={1.6} />
            </button>
            <button
              type="submit"
              class="send"
              aria-label={m.work_env_send()}
              disabled={disabled || busy}
              inert={!writing}
            >
              <Icon icon={ArrowUp02Icon} size={16} strokeWidth={2.2} />
            </button>
          </div>
        </div>
      </div>
    </form>
    <svg class="edge" aria-hidden="true">
      <defs>
        <linearGradient id="{id}-edge" x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" class="line-stop" />
          <stop offset="0.35" class="line-stop" stop-opacity="0.85" />
          <stop offset="1" class="line-stop" stop-opacity="0.12" />
        </linearGradient>
      </defs>
      <path class="body" />
      <path class="line" stroke="url(#{id}-edge)" />
    </svg>
  </div>
</div>

<style>
  /* One thin sheet on the canvas's bottom edge. At rest it holds the canvas's
     own tools and the ask; in use it is the ask alone. The words sit on the
     sheet itself: there is no field inside the bar, the bar is the field. The
     run's line is not here; it stands at the canvas's top. */
  .work-bar {
    --bar-width: 620px;

    display: flex;
    flex-direction: column;
    align-items: stretch;
    gap: 10px;
    box-sizing: border-box;
    inline-size: min(var(--bar-width), calc(100% - 48px));
    pointer-events: none;
    contain: layout style;
    transition: inline-size var(--motion-slow) var(--ease-emphasized);
  }

  .work-bar[data-mode="focus"] {
    --bar-width: 700px;
  }

  .above,
  .surface {
    pointer-events: auto;
  }

  .above {
    display: flex;
    flex-direction: column;
    gap: 8px;
    max-block-size: min(48vh, 480px);
    overflow: auto;
    padding-inline: 4px;
  }

  .surface {
    --flare: 20px;
    --edge: var(--color-border-strong);
    --fill: var(--color-chrome);

    position: relative;
    display: grid;
    grid-template-columns: auto minmax(0, 1fr);
    grid-template-areas: "tools field";
    align-items: end;
    gap: 4px 0;
    box-sizing: border-box;
    padding: 8px 8px 10px;
    border-radius: var(--radius-panel) var(--radius-panel) 0 0;
    isolation: isolate;
    box-shadow: var(--shadow-sheet);
    transition: grid-template-columns var(--motion-slow) var(--ease-emphasized);
  }

  :global(:root:not([data-material="none"])[data-material]) .surface {
    --fill: transparent;
  }

  /* The sheet is one drawn shape: its fill and its hairline follow the outline the canvas cuts its
     hole to (see composer-hole.ts), fillets down to the card's foot included, so the fill, the
     hole and the line cannot part. Over a material the fill is nothing and the window shows. */
  .edge {
    position: absolute;
    inset: 0;
    z-index: -1;
    inline-size: 100%;
    block-size: 100%;
    overflow: visible;
    pointer-events: none;
  }

  .body {
    fill: var(--fill);
  }

  .line {
    fill: none;
    stroke-width: 1;
  }

  .line-stop {
    stop-color: var(--edge);
  }

  .tools {
    grid-area: tools;
    display: grid;
    grid-template-columns: 1fr;
    align-self: center;
    opacity: 1;
    transition:
      grid-template-columns var(--motion-slow) var(--ease-emphasized),
      opacity var(--motion-fast) var(--ease-out);
  }

  .work-bar[data-mode="focus"] .tools {
    grid-template-columns: 0fr;
    opacity: 0;
    transition-timing-function: var(--ease-exit), var(--ease-exit);
  }

  .tools-inner {
    display: flex;
    align-items: center;
    gap: 0;
    min-inline-size: 0;
    overflow: hidden;
  }

  /* At rest nothing folds, so the tools' hints may rise above the bar. */
  .work-bar[data-mode="rest"] .tools-inner {
    overflow: visible;
  }

  .rule {
    flex: none;
    inline-size: 1px;
    block-size: 18px;
    margin-inline: 6px 4px;
    background: var(--color-border);
  }

  .field {
    grid-area: field;
    display: flex;
    flex-direction: column;
    gap: 6px;
    min-inline-size: 0;
    margin: 0;
    padding: 0 0 0 8px;
  }

  .work-bar[data-engaged="true"] .field {
    padding-inline-start: 0;
  }

  /* What goes with the ask, as chips at the head of the field. */
  .context {
    display: flex;
    flex: none;
    align-items: center;
    align-self: center;
    gap: 6px;
  }

  .row {
    display: flex;
    align-items: flex-end;
    gap: 4px;
    min-inline-size: 0;
  }

  /* The attach button opens from nothing beside the words once the tools
     have folded away, and closes back into them. */
  .side {
    display: grid;
    grid-template-columns: 0fr;
    flex: none;
    opacity: 0;
    transition:
      grid-template-columns var(--motion-slow) var(--ease-emphasized),
      opacity var(--motion-fast) var(--ease-out);
  }

  .work-bar[data-engaged="true"] .side {
    grid-template-columns: 1fr;
    opacity: 1;
  }

  .side-inner {
    display: flex;
    align-items: center;
    min-inline-size: 0;
    overflow: hidden;
  }

  .trail {
    display: flex;
    flex: none;
    align-items: center;
    gap: 2px;
  }

  textarea {
    flex: 1;
    min-inline-size: 0;
    box-sizing: border-box;
    min-block-size: 34px;
    max-block-size: 132px;
    padding: 7px 4px;
    border: 0;
    background: transparent;
    color: var(--color-text);
    font: inherit;
    font-size: 14px;
    line-height: 20px;
    resize: none;
    outline: none;
  }

  textarea::placeholder {
    color: var(--color-faint);
  }

  .model {
    max-inline-size: 140px;
    padding-inline: 8px;
    overflow: hidden;
    color: var(--color-muted);
    font-size: var(--text-label);
    line-height: 34px;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  /* The mic and Send share one place: whichever the field needs is there, the other gone. */
  .action {
    display: grid;
    flex: none;
    place-items: center;
  }

  .mic,
  .send {
    display: grid;
    grid-area: 1 / 1;
    place-items: center;
    inline-size: 30px;
    block-size: 30px;
    margin: 2px;
    border: 0;
    border-radius: var(--radius-capsule);
    cursor: default;
    transition:
      background-color var(--motion-fast) var(--ease-out),
      color var(--motion-fast) var(--ease-out),
      opacity var(--motion-base) var(--ease-out),
      scale var(--motion-base) var(--ease-spring);
  }

  .mic {
    background: transparent;
    color: var(--color-muted);
  }

  .send {
    background: var(--color-lit);
    color: var(--color-on-lit);
    opacity: 0;
    scale: 0.6;
  }

  .mic:focus-visible,
  .send:focus-visible {
    outline: 2px solid var(--color-ring);
    outline-offset: 2px;
  }

  .mic:disabled {
    color: var(--color-faint);
  }

  .mic:hover:not(:disabled) {
    background: var(--color-fill-hover);
    color: var(--color-text);
  }

  .row[hidden] {
    display: none;
  }

  .send:disabled {
    background: var(--color-fill);
    color: var(--color-faint);
  }

  .send:hover:not(:disabled) {
    background: var(--color-lit-hover);
  }

  .send:active:not(:disabled) {
    scale: 0.92;
    transition-duration: var(--motion-instant);
  }

  .action[data-send="true"] .mic {
    opacity: 0;
    scale: 0.6;
  }

  .action[data-send="true"] .send {
    opacity: 1;
    scale: 1;
  }

  .sr-only {
    position: absolute;
    inline-size: 1px;
    block-size: 1px;
    overflow: hidden;
    clip-path: inset(50%);
  }

  @media (prefers-reduced-motion: reduce) {
    .mic,
    .send {
      transition: opacity var(--motion-fast) linear;
    }
  }

  @media (forced-colors: active) {
    .line {
      stroke: ButtonText;
    }
  }
</style>
