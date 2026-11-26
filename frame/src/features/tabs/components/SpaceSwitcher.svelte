<script lang="ts">
  import * as m from "$shared/i18n/messages";
  import "$shared/ui/Shine/shine.css";
  import { Add01Icon, Globe02Icon, PencilEdit02Icon } from "@hugeicons/core-free-icons";
  import type { SpaceView } from "$shared/ipc/bindings";
  import { tabs } from "$domain/tabs";
  import { favicons } from "$domain/favicons";
  import Icon from "$shared/ui/Icon";
  import FavIcon from "$shared/ui/FavIcon";
  import SpaceGlyph from "./SpaceGlyph.svelte";
  import { spaceKeys } from "../lib/space-keys";

  let {
    spaces,
    active,
    open = true,
    onchoose,
    making = false,
    failed = false,
    onedit,
    onnew,
  }: {
    spaces: SpaceView[];
    active: string | null;
    /** The panel is showing, which is when the keys are listened for. */
    open?: boolean;
    /** A new space was asked for and has not appeared yet. */
    making?: boolean;
    failed?: boolean;
    onchoose: (id: string) => void;
    onedit: (space: SpaceView, rename?: boolean) => void;
    onnew: () => void;
  } = $props();

  let list = $state<HTMLElement>();
  let held = $state<{ id: string; pointerId: number; from: number; y: number; to: number } | null>(
    null,
  );
  let moved = $state(false);

  // Holding Control names the key of every row, as a menu's alternates appear.
  let ctrl = $state(false);
  $effect(() => {
    if (!open) {
      ctrl = false;
      held = null;
      moved = false;
      return;
    }
    const read = (event: KeyboardEvent) => (ctrl = event.ctrlKey);
    const lost = () => {
      ctrl = false;
      held = null;
    };
    window.addEventListener("keydown", read, true);
    window.addEventListener("keyup", read, true);
    window.addEventListener("blur", lost);
    // Before a row captures the pointer it may be released anywhere in the window.
    window.addEventListener("pointerup", cancel);
    window.addEventListener("pointercancel", cancel);
    return () => {
      window.removeEventListener("keydown", read, true);
      window.removeEventListener("keyup", read, true);
      window.removeEventListener("blur", lost);
      window.removeEventListener("pointerup", cancel);
      window.removeEventListener("pointercancel", cancel);
      ctrl = false;
    };
  });

  // Rows reorder by dragging them, or with Option and the arrows.
  function down(event: PointerEvent, id: string, from: number) {
    if (event.button !== 0 || (held && held.pointerId !== event.pointerId)) return;
    held = { id, pointerId: event.pointerId, from, y: event.clientY, to: from };
    moved = false;
  }

  function move(event: PointerEvent) {
    if (!held || held.pointerId !== event.pointerId || !list) return;
    if (event.pointerType === "mouse" && (event.buttons & 1) === 0) {
      held = null;
      return;
    }
    if (!moved && Math.abs(event.clientY - held.y) < 5) return;
    if (!moved) (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
    moved = true;
    const rows = [...list.querySelectorAll<HTMLElement>("[data-space-row]")];
    const to = rows.findIndex((row) => {
      const rect = row.getBoundingClientRect();
      return event.clientY < rect.top + rect.height / 2;
    });
    held = { ...held, to: to === -1 ? rows.length : to };
  }

  function up(event: PointerEvent) {
    if (!held || held.pointerId !== event.pointerId) return;
    const { id, from, to } = held;
    held = null;
    if (!moved || to === from || to === from + 1) return;
    void tabs.reorderSpace(id, spaces[to]?.id ?? null);
  }

  function cancel(event: PointerEvent) {
    if (held?.pointerId === event.pointerId) held = null;
  }

  function choose(event: MouseEvent, id: string) {
    if (held) return;
    if (moved && event.detail > 0) {
      moved = false;
      return;
    }
    onchoose(id);
  }

  function keydown(event: KeyboardEvent, index: number) {
    if (!event.altKey || (event.key !== "ArrowUp" && event.key !== "ArrowDown")) return;
    event.preventDefault();
    event.stopPropagation();
    const id = spaces[index]!.id;
    if (event.key === "ArrowUp" && index > 0) void tabs.reorderSpace(id, spaces[index - 1]!.id);
    if (event.key === "ArrowDown" && index < spaces.length - 1)
      void tabs.reorderSpace(id, spaces[index + 2]?.id ?? null);
  }
</script>

<div
  class="switcher"
  role="menu"
  aria-label={m.space_switcher()}
  bind:this={list}
  data-keys={ctrl || undefined}
>
  {#each spaces as space, index (space.id)}
    {@const current = space.id === active}
    {@const keys = spaceKeys(index)}
    {@const shown = Math.min(space.tabs, 3)}
    <div
      role="presentation"
      class="switch-row"
      class:ui-shine={current}
      data-space-row
      data-current={current || undefined}
      data-held={held?.id === space.id && moved ? "" : undefined}
      data-drop-before={held && moved && held.to === index && held.from !== index ? "" : undefined}
      data-drop-after={held && moved && held.to === spaces.length && index === spaces.length - 1
        ? ""
        : undefined}
      oncontextmenu={(event) => {
        event.preventDefault();
        onedit(space, true);
      }}
    >
      <button
        type="button"
        role="menuitemradio"
        class="ui-menu-item switch-pick"
        aria-checked={current}
        aria-keyshortcuts={keys ?? undefined}
        onpointerdown={(event) => down(event, space.id, index)}
        onpointermove={move}
        onpointerup={up}
        onlostpointercapture={cancel}
        onkeydown={(event) => keydown(event, index)}
        onclick={(event) => choose(event, space.id)}
      >
        <SpaceGlyph icon={space.icon} color={space.color} size={16} />
        <span class="switch-name">{space.name}</span>
        <span class="switch-tail">
          {#if shown > 0}<span class="switch-stack" aria-hidden="true"
              >{#each { length: shown }, at (at)}{@const ref = space.previews[at] ?? null}<span
                  class="switch-fav"
                  style:--at={at}
                  ><FavIcon
                    image={favicons.image(ref)}
                    tone={favicons.tone(ref)}
                    size={16}
                    fallback={Globe02Icon}
                  /></span
                >{/each}{#if space.tabs > 3}<span class="switch-more">+{space.tabs - 3}</span
                >{/if}</span
            >{/if}
          {#if keys}<span class="switch-keys" aria-hidden="true">{keys}</span>{/if}
        </span>
      </button>
      <button
        type="button"
        class="switch-edit"
        title={m.space_edit_label({ name: space.name })}
        aria-label={m.space_edit_label({ name: space.name })}
        onclick={() => onedit(space)}><Icon icon={PencilEdit02Icon} size={13} /></button
      >
    </div>
  {/each}
  <div class="switch-rule" role="separator"></div>
  <button
    type="button"
    role="menuitem"
    class="ui-menu-item switch-new"
    aria-disabled={making || undefined}
    onclick={() => !making && onnew()}
  >
    <Icon icon={Add01Icon} size={16} strokeWidth={1.8} />
    <span class="switch-name">{m.space_new()}</span>
  </button>
  {#if failed}<p class="switch-error" role="alert">{m.space_new_failed()}</p>{/if}
</div>

<style>
  .switcher {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }

  .switch-row {
    --shine-radius: calc(var(--radius-menu) - var(--menu-pad));

    position: relative;
    display: flex;
    align-items: center;
    border-radius: var(--shine-radius);
    transition: background-color var(--motion-instant) var(--ease-smooth);
  }

  .switch-row:not([data-current]):is(:hover, :focus-within) {
    background: var(--row-hover);
  }

  .switch-pick,
  .switch-new {
    flex: 1;
    min-width: 0;
    width: 100%;
    min-height: 34px;
    padding: 5px 10px;
    border: 0;
    background: transparent;
    font: inherit;
    font-size: var(--text-body);
    text-align: start;
  }

  .switch-new {
    color: var(--color-muted);
    padding-inline-start: 10px;
  }

  .switch-pick:focus-visible,
  .switch-new:focus-visible {
    outline: none;
    background: var(--row-hover);
  }

  .switch-new:hover,
  .switch-new:focus-visible {
    background: var(--row-hover);
    color: var(--color-text);
  }

  .switch-new[aria-disabled] {
    opacity: 0.55;
  }

  .switch-pick:active,
  .switch-new:active {
    background: var(--row-pressed);
  }

  .switch-name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    line-height: 18px;
  }

  /* The stack and the keys share one place; holding Control swaps them. */
  .switch-tail {
    display: grid;
    flex: none;
    place-items: center end;
  }

  .switch-tail > * {
    grid-area: 1 / 1;
    transition: opacity var(--motion-fast) var(--ease-out);
  }

  .switch-stack {
    display: flex;
    align-items: center;
  }

  /* Each icon sits over the one after it, ringed in the panel's own ground. */
  .switch-fav {
    display: grid;
    border-radius: 5px;
    background: var(--color-float);
    box-shadow: 0 0 0 1.5px var(--color-float);
    z-index: calc(4 - var(--at));
  }

  .switch-fav + .switch-fav {
    margin-inline-start: -6px;
  }

  .switch-more {
    margin-inline-start: 5px;
    color: var(--color-faint);
    font-size: var(--text-label);
    font-variant-numeric: tabular-nums;
  }

  .switch-keys {
    color: var(--color-faint);
    font-size: var(--text-label);
    font-variant-numeric: tabular-nums;
    letter-spacing: 0.02em;
    opacity: 0;
  }

  .switcher[data-keys] .switch-keys {
    opacity: 1;
  }

  .switcher[data-keys] .switch-stack {
    opacity: 0;
  }

  /* The pencil takes the stack's place on approach: the stack fades out as it
     fades in, so a row at rest has no hole. */
  .switch-edit {
    position: absolute;
    inset-block: 0;
    inset-inline-end: 4px;
    display: grid;
    width: 24px;
    height: 24px;
    margin-block: auto;
    padding: 0;
    border: 0;
    border-radius: var(--radius-control-compact);
    background: transparent;
    color: var(--color-muted);
    opacity: 0;
    place-items: center;
    pointer-events: none;
    transition:
      opacity var(--motion-fast) var(--ease-out),
      background-color var(--motion-instant) var(--ease-out),
      color var(--motion-instant) var(--ease-out);
  }

  .switch-edit:hover,
  .switch-edit:focus-visible {
    outline: none;
    background: var(--row-pressed);
    color: var(--color-text);
    opacity: 1;
    pointer-events: auto;
  }

  .switch-row:is(:hover, :focus-within) .switch-edit {
    opacity: 1;
    pointer-events: auto;
  }

  .switch-row:is(:hover, :focus-within) .switch-stack {
    opacity: 0;
  }

  .switcher[data-keys] .switch-edit {
    opacity: 0;
    pointer-events: none;
  }

  .switch-row[data-held] {
    opacity: 0.55;
  }

  .switch-row[data-drop-before]::before,
  .switch-row[data-drop-after]::after {
    position: absolute;
    height: 2px;
    border-radius: var(--radius-capsule);
    background: var(--color-text);
    content: "";
    inset-inline: 10px;
  }

  .switch-row[data-drop-before]::before {
    inset-block-start: -2px;
  }

  .switch-row[data-drop-after]::after {
    inset-block-end: -2px;
  }

  .switch-rule {
    height: 1px;
    margin: 4px var(--menu-item-inset);
    background: var(--color-border);
  }

  .switch-error {
    padding: 2px var(--menu-item-inset) 4px;
    color: var(--color-danger);
    font-size: var(--text-label);
    line-height: 16px;
  }

  @media (prefers-reduced-motion: reduce) {
    .switch-edit,
    .switch-row,
    .switch-tail > * {
      transition: none;
    }
  }
</style>
