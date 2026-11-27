<script lang="ts" module>
  export type LauncherAction = { id: string; label: string; keys: string[]; run: () => void };
  /** The sheet's id, which the field names while the sheet is open. */
  export const ACTIONS_ID = "launcher-actions";
</script>

<script lang="ts">
  import * as m from "$shared/i18n/messages";
  import KeyHint from "$shared/ui/KeyHint";

  let {
    actions,
    height = $bindable(0),
    onhighlight,
    onrun,
    onclose,
  }: {
    actions: LauncherAction[];
    height?: number;
    /** The highlighted item's id, for the field's `aria-activedescendant`. */
    onhighlight: (id: string | undefined) => void;
    onrun: (action: LauncherAction) => void;
    onclose: () => void;
  } = $props();

  let index = $state(0);
  let sheet = $state<HTMLElement>();
  const itemId = (action: LauncherAction) => `${ACTIONS_ID}-${action.id}`;

  $effect(() => {
    const action = actions[Math.min(index, actions.length - 1)];
    onhighlight(action ? itemId(action) : undefined);
  });

  // The field keeps focus throughout, so the sheet reads keys from the window
  // rather than taking focus and handing it back.
  function keydown(event: KeyboardEvent) {
    if (event.defaultPrevented || !actions.length) return;
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      const step = event.key === "ArrowDown" ? 1 : -1;
      index = Math.min(actions.length - 1, Math.max(0, index + step));
    } else if (event.key === "Enter") {
      event.preventDefault();
      const action = actions[Math.min(index, actions.length - 1)];
      if (action) onrun(action);
    }
  }

  function pointerdown(event: PointerEvent) {
    const target = event.target as Element | null;
    if (!sheet?.contains(target) && !target?.closest('button[aria-haspopup="menu"]')) onclose();
  }
</script>

<svelte:window onkeydown={keydown} onpointerdown={pointerdown} />

<div
  id={ACTIONS_ID}
  class="sheet"
  role="menu"
  aria-label={m.launcher_actions()}
  bind:this={sheet}
  bind:offsetHeight={height}
>
  {#each actions as action, i (action.id)}
    <button
      type="button"
      id={itemId(action)}
      role="menuitem"
      tabindex="-1"
      class:active={i === index}
      onpointermove={() => (index = i)}
      onpointerdown={(event) => event.preventDefault()}
      onclick={() => onrun(action)}
      ><span>{action.label}</span><KeyHint keys={action.keys} /></button
    >
  {/each}
</div>

<style>
  .sheet {
    position: absolute;
    inset-block-end: 48px;
    inset-inline-end: 10px;
    z-index: 1;
    display: flex;
    flex-direction: column;
    min-width: 248px;
    padding: 6px;
    border: 1px solid var(--color-border-strong);
    border-radius: var(--radius-card);
    background: var(--color-raised);
    box-shadow: var(--shadow-popover);
    transform-origin: bottom right;
    animation: sheet-in var(--motion-fast) var(--ease-out);
  }

  button {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
    height: 32px;
    padding: 0 6px 0 10px;
    border: 0;
    border-radius: var(--radius-inset);
    background: transparent;
    color: var(--color-text);
    font-size: 13px;
    text-align: start;
  }

  button.active {
    background: var(--color-fill-active);
  }

  @keyframes sheet-in {
    from {
      opacity: 0;
      transform: translateY(4px) scale(0.98);
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .sheet {
      animation: none;
    }
  }

  @media (forced-colors: active) {
    .sheet {
      border-color: CanvasText;
    }

    button.active {
      outline: 1px solid Highlight;
    }
  }
</style>
