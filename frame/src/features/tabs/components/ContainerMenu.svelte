<script lang="ts">
  import "$shared/ui/Menu/popover.css";
  import {
    Add01Icon,
    Cancel01Icon,
    PencilEdit02Icon,
    ShieldKeyIcon,
  } from "@hugeicons/core-free-icons";
  import Icon from "$shared/ui/Icon";
  import type { ContainerView, SpaceIconView } from "$shared/ipc/bindings";
  import { tabs as tabDomain } from "$domain/tabs";
  import { SPACE_COLORS, SPACE_SYMBOLS } from "../lib/space-look";

  let open = $state(false);
  let trigger: HTMLButtonElement | undefined = $state();
  let root: HTMLDivElement;
  let editing = $state<string | "new" | null>(null);
  let name = $state("");
  let color = $state(2);
  let icon = $state<SpaceIconView | null>({ kind: "symbol", index: 1 });
  let selectedSpaces = $state<string[]>([]);
  let busy = $state(false);
  let operationError = $state(false);
  let removeRetry = $state<ContainerView | null>(null);
  let error = $state(false);
  let previousSpace = $state<string | null>(null);
  const currentSpace = $derived(tabDomain.activeSpaceId());
  const currentTab = $derived(tabDomain.activeTab());
  const containers = $derived(tabDomain.containersList());
  const spaces = $derived(tabDomain.spaces());
  const availableContainers = $derived(
    containers.filter((item) => item.spaces === null || item.spaces.includes(currentSpace ?? "")),
  );
  const selected = (id: string) => currentTab?.container_id === id;

  $effect(() => {
    const activeSpace = currentSpace;
    if (previousSpace !== activeSpace) {
      previousSpace = activeSpace;
      removeRetry = null;
      operationError = false;
    }
  });

  function onWindowKeydown(event: KeyboardEvent) {
    if (open && event.key === "Escape") {
      event.preventDefault();
      open = false;
      trigger?.focus();
    }
  }
  $effect(() => {
    if (!open) return;
    const outside = (event: PointerEvent) => {
      if (root && event.target instanceof Node && !root.contains(event.target)) open = false;
    };
    window.addEventListener("pointerdown", outside, true);
    return () => window.removeEventListener("pointerdown", outside, true);
  });

  function beginEdit(container?: ContainerView) {
    error = false;
    editing = container?.id ?? "new";
    name = container?.name ?? "";
    color = container?.color ?? 2;
    icon = container?.icon ?? { kind: "symbol", index: 1 };
    selectedSpaces = container?.spaces ? [...container.spaces] : spaces.map((space) => space.id);
  }
  async function save() {
    if (busy || !name.trim()) return;
    busy = true;
    error = false;
    try {
      let result;
      const scope = selectedSpaces.length === spaces.length ? null : [...selectedSpaces];
      if (editing === "new")
        result = await tabDomain.createContainer(name.trim(), icon, color, scope);
      else {
        const container = containers.find((item) => item.id === editing);
        if (container)
          result = await tabDomain.updateContainer(container, name.trim(), icon, color, scope);
      }
      if (result?.outcome === "applied" || result?.outcome === "deferred") editing = null;
      else error = true;
    } catch {
      error = true;
    } finally {
      busy = false;
    }
  }
  async function remove(container: ContainerView) {
    removeRetry = null;
    operationError = false;
    try {
      const result = await tabDomain.removeContainerFromSpace(container.id);
      if (result.outcome !== "applied" && result.outcome !== "deferred") {
        removeRetry = container;
        operationError = true;
      }
    } catch {
      removeRetry = container;
      operationError = true;
    }
  }
  async function assign(container: string | null) {
    if (!currentTab) return;
    operationError = false;
    try {
      const result = await tabDomain.assignTabContainer(currentTab.id, container);
      if (result.outcome !== "applied" && result.outcome !== "deferred") operationError = true;
    } catch {
      operationError = true;
    }
  }
  function radioKeys(event: KeyboardEvent) {
    if (!["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Home", "End"].includes(event.key))
      return;
    const group = event.currentTarget as HTMLElement;
    const radios = [...group.querySelectorAll<HTMLButtonElement>('[role="radio"]')];
    const current = radios.indexOf(event.target as HTMLButtonElement);
    if (current < 0 || radios.length === 0) return;
    const next =
      event.key === "Home"
        ? 0
        : event.key === "End"
          ? radios.length - 1
          : (current + (["ArrowRight", "ArrowDown"].includes(event.key) ? 1 : -1) + radios.length) %
            radios.length;
    event.preventDefault();
    radios[next]?.click();
    radios[next]?.focus();
  }
</script>

<svelte:window onkeydown={onWindowKeydown} />
<div bind:this={root} class="container-tools" data-open={open} data-space-hue="0">
  <button
    bind:this={trigger}
    class="container-trigger"
    type="button"
    aria-haspopup="dialog"
    aria-expanded={open}
    data-state={open ? "open" : "closed"}
    onclick={() => {
      if (!open) editing = null;
      open = !open;
    }}
  >
    <Icon icon={ShieldKeyIcon} size={15} />
    <span>Containers</span>
    {#if containers.length}<span class="count">{containers.length}</span>{/if}
  </button>
  <div
    class="ui-menu ui-disclosure-panel ui-popover container-popover sidebar-softens"
    data-menu-material="opaque"
    data-open={open}
    data-align="start"
    role="dialog"
    aria-label="Browsing containers"
    inert={!open}
  >
    {#if editing !== null}
      <div class="editor-head">
        <button type="button" class="back" onclick={() => (editing = null)}>‹</button><strong
          >{editing === "new" ? "New container" : "Edit container"}</strong
        >
      </div>
      <label class="name-label"
        >Name<input bind:value={name} maxlength="48" placeholder="e.g. Personal" /></label
      >
      <div class="group-label">Icon</div>
      <div
        class="symbols"
        role="radiogroup"
        tabindex="-1"
        aria-label="Container icon"
        onkeydown={radioKeys}
      >
        {#each SPACE_SYMBOLS as symbol, index (index)}
          <button
            type="button"
            role="radio"
            tabindex={icon?.kind === "symbol"
              ? icon.index === index
                ? 0
                : -1
              : index === 0
                ? 0
                : -1}
            aria-checked={icon?.kind === "symbol" && icon.index === index}
            aria-label={`Icon ${index + 1}`}
            onclick={() => (icon = { kind: "symbol", index })}
            ><Icon icon={symbol} size={14} /></button
          >
        {/each}
      </div>
      <label class="emoji-label"
        >Emoji<input
          aria-label="Custom emoji"
          maxlength="16"
          value={icon?.kind === "emoji" ? icon.text : ""}
          oninput={(event) =>
            (icon = event.currentTarget.value
              ? { kind: "emoji", text: event.currentTarget.value }
              : null)}
          placeholder="Optional"
        /></label
      >
      <div class="group-label">Color</div>
      <div
        class="colors"
        role="radiogroup"
        tabindex="-1"
        aria-label="Container color"
        onkeydown={radioKeys}
      >
        {#each Array.from({ length: SPACE_COLORS }, (_, i) => i) as hue (hue)}
          <button
            class="swatch"
            type="button"
            tabindex={color === hue ? 0 : -1}
            data-space-hue={hue}
            role="radio"
            aria-checked={color === hue}
            aria-label={`Color ${hue + 1}`}
            onclick={() => (color = hue)}
          ></button>
        {/each}
      </div>
      <div class="group-label">Available in Spaces</div>
      <div class="space-access">
        {#each spaces as space (space.id)}
          <label
            ><input
              type="checkbox"
              checked={selectedSpaces.includes(space.id)}
              onchange={(event) =>
                (selectedSpaces = event.currentTarget.checked
                  ? [...selectedSpaces, space.id]
                  : selectedSpaces.filter((id) => id !== space.id))}
            /><span data-space-hue={space.color}></span>{space.name}</label
          >
        {/each}
      </div>
      {#if error}<p class="error" role="alert">Couldn’t save the container. Try again.</p>{/if}
      <button class="save" type="button" disabled={busy || !name.trim()} onclick={save}
        >{editing === "new" ? "Create container" : "Save changes"}</button
      >
    {:else}
      <div class="popover-title">
        <div><strong>Browsing containers</strong><span>Separate site sessions</span></div>
        <button
          class="add"
          type="button"
          aria-label="Add container"
          title="Add container"
          onclick={() => beginEdit()}><Icon icon={Add01Icon} size={16} /></button
        >
      </div>
      <div class="container-options" role="group" aria-label="Assign current tab">
        <button
          class="container-option default"
          type="button"
          aria-pressed={!currentTab?.container_id}
          onclick={() => void assign(null)}
        >
          <span class="dot"></span><span>Default browsing</span>{#if !currentTab?.container_id}<span
              class="check">✓</span
            >{/if}
        </button>
        {#each availableContainers as container (container.id)}
          <div class="container-option-wrap" data-space-hue={container.color}>
            <button
              class="container-option"
              type="button"
              aria-pressed={selected(container.id)}
              onclick={() => void assign(container.id)}
            >
              <span class="dot"
                ><Icon
                  icon={SPACE_SYMBOLS[
                    container.icon?.kind === "symbol" ? container.icon.index : 0
                  ] ?? ShieldKeyIcon}
                  size={13}
                /></span
              ><span class="option-name"
                >{container.icon?.kind === "emoji"
                  ? `${container.icon.text} `
                  : ""}{container.name}</span
              >{#if selected(container.id)}<span class="check">✓</span>{/if}
            </button>
            <button
              class="edit"
              type="button"
              title={`Edit ${container.name}`}
              aria-label={`Edit ${container.name}`}
              onclick={() => beginEdit(container)}
              ><Icon icon={PencilEdit02Icon} size={13} /></button
            >
            <button
              class="remove"
              type="button"
              title={`Remove ${container.name} from this Space`}
              aria-label={`Remove ${container.name} from this Space`}
              onclick={() => void remove(container)}><Icon icon={Cancel01Icon} size={13} /></button
            >
          </div>
        {/each}
      </div>
      {#if operationError}
        <p class="error" role="alert">
          Couldn’t update this container. Try again.
          {#if removeRetry}<button type="button" onclick={() => void remove(removeRetry!)}
              >Retry</button
            >{/if}
        </p>
      {/if}
      {#if !currentTab}<p class="hint">Open a tab to assign it to a container.</p>{/if}
    {/if}
  </div>
</div>

<style>
  @import "./space-hue.css";

  :global {
    .container-tools {
      position: relative;
      z-index: 30;
      width: 100%;
      padding: 0 5px 5px;
    }

    .container-trigger {
      display: flex;
      align-items: center;
      gap: 8px;
      width: 100%;
      min-height: 30px;
      padding: 0 10px;
      border: 1px solid transparent;
      border-radius: var(--radius-row);
      background: transparent;
      color: var(--color-label-secondary);
      font: inherit;
      font-size: var(--sidebar-row-text);
      font-weight: 550;
      text-align: left;
      cursor: default;
      transition:
        background-color var(--motion-fast) var(--ease-out),
        color var(--motion-fast) var(--ease-out);
    }

    .container-trigger:hover,
    .container-trigger[aria-expanded="true"] {
      color: var(--color-text);
      background: var(--row-hover);
    }

    .count {
      margin-inline-start: auto;
      color: var(--color-faint);
      font-variant-numeric: tabular-nums;
    }

    .container-popover {
      inset-inline: var(--sidebar-inset);
      width: auto;
      min-width: 0;
      max-width: none;
      max-height: min(560px, calc(100vh - 200px));
      overflow-y: auto;
      padding: var(--menu-pad);
      color: var(--color-text);
      background: var(--color-float);
      backdrop-filter: none;
      transform-origin: 44px -10px;
    }

    .popover-title,
    .editor-head {
      position: relative;
      display: flex;
      align-items: center;
      justify-content: space-between;
      gap: 12px;
      padding: 2px 36px 12px 3px;
    }

    .editor-head {
      justify-content: flex-start;
      padding-right: 3px;
    }

    .popover-title div {
      display: grid;
      min-width: 0;
      gap: 4px;
    }

    .popover-title strong,
    .editor-head strong {
      font-size: 12px;
      font-weight: 650;
      letter-spacing: -0.01em;
      white-space: nowrap;
    }

    .popover-title span {
      color: var(--color-faint);
      font-size: 10px;
      line-height: 14px;
    }

    .add,
    .back,
    .edit,
    .remove {
      display: grid;
      place-items: center;
      width: 30px;
      height: 30px;
      flex: none;
      border: 0;
      border-radius: 50%;
      background: transparent;
      color: var(--color-muted);
      cursor: default;
      transition:
        background-color var(--motion-fast) var(--ease-out),
        color var(--motion-fast) var(--ease-out);
    }

    .add {
      position: absolute;
      inset-block-start: 0;
      inset-inline-end: 0;
    }

    .add:hover,
    .back:hover,
    .edit:hover,
    .remove:hover {
      background: var(--row-hover);
      color: var(--color-text);
    }

    .container-options {
      display: grid;
      gap: 4px;
      max-height: 300px;
      overflow: auto;
    }

    .container-option-wrap {
      display: flex;
      align-items: center;
      position: relative;
      gap: 0;
      padding: 0 1px 0 0;
      border-radius: var(--radius-row);
    }

    .container-option-wrap:hover {
      background: var(--row-hover);
    }

    .container-option {
      display: flex;
      align-items: center;
      gap: 8px;
      flex: 1;
      min-width: 0;
      min-height: 42px;
      padding: 6px 7px;
      border: 0;
      border-radius: var(--radius-row);
      background: transparent;
      color: var(--color-text);
      font: inherit;
      font-size: var(--text-body);
      text-align: left;
      cursor: default;
    }

    .container-option[aria-pressed="true"] {
      background: var(--row-active);
    }

    .dot {
      display: grid;
      place-items: center;
      width: 24px;
      height: 24px;
      flex: none;
      border-radius: var(--radius-inset);
      color: var(--space-hue);
      background: color-mix(in srgb, var(--space-hue) 24%, var(--color-float));
    }

    .option-name {
      display: block;
      position: relative;
      z-index: 1;
      flex: 1;
      min-width: 0;
      overflow: hidden;
      color: var(--color-text);
      opacity: 1;
      white-space: nowrap;
      text-overflow: ellipsis;
    }

    .check {
      color: var(--color-muted);
      font-weight: 650;
    }

    .container-option-wrap .edit,
    .container-option-wrap .remove {
      display: none;
      width: 24px;
      height: 24px;
    }

    .container-option-wrap:hover .edit,
    .container-option-wrap:hover .remove,
    .container-option-wrap:focus-within .edit,
    .container-option-wrap:focus-within .remove {
      display: grid;
    }

    @media (hover: none) {
      .container-option-wrap .edit,
      .container-option-wrap .remove {
        display: grid;
      }
    }

    .default .dot {
      display: none;
    }

    .hint {
      margin: 9px 4px 1px;
      color: var(--color-faint);
      font-size: 11px;
    }

    .error {
      margin: 8px 3px 0;
      color: var(--color-danger);
      font-size: 10px;
    }

    .name-label,
    .emoji-label {
      display: grid;
      gap: 5px;
      margin: 0 2px 10px;
      color: var(--color-label-secondary);
      font-size: 10px;
    }

    .name-label input,
    .emoji-label input {
      height: 31px;
      padding: 0 9px;
      border: 1px solid var(--color-separator);
      border-radius: var(--radius-control-compact);
      outline-color: var(--color-accent);
      background: var(--color-surface-raised);
      color: var(--color-text);
      font: inherit;
      font-size: 11px;
    }

    .group-label {
      margin: 9px 3px 5px;
      color: var(--color-label-secondary);
      font-size: 10px;
    }

    .symbols,
    .colors {
      display: flex;
      flex-wrap: wrap;
      gap: 4px;
      padding: 0 2px;
    }

    .symbols button {
      display: grid;
      place-items: center;
      width: 25px;
      height: 25px;
      border: 1px solid transparent;
      border-radius: var(--radius-inset);
      background: transparent;
      color: var(--color-label-secondary);
    }

    .symbols button[aria-checked="true"] {
      border-color: var(--color-accent);
      color: var(--color-text);
      background: var(--row-hover);
    }

    .swatch {
      width: 18px;
      height: 18px;
      border: 1px solid color-mix(in srgb, var(--color-text) 12%, transparent);
      border-radius: 50%;
      background: var(--space-hue);
    }

    .swatch[aria-checked="true"] {
      outline: 2px solid var(--color-text);
      outline-offset: 2px;
    }

    .save {
      width: 100%;
      height: 31px;
      margin-top: 13px;
      border: 0;
      border-radius: var(--radius-inset);
      background: var(--color-accent);
      color: var(--color-accent-contrast, white);
      font: inherit;
      font-size: 11px;
      font-weight: 600;
    }

    .save:disabled {
      opacity: 0.45;
    }

    .space-access {
      display: grid;
      gap: 2px;
      max-height: 90px;
      overflow: auto;
    }

    .space-access label {
      display: flex;
      align-items: center;
      gap: 7px;
      min-height: 23px;
      color: var(--color-label-secondary);
      font-size: 10px;
    }

    .space-access input {
      accent-color: var(--color-accent);
    }

    .space-access span {
      width: 7px;
      height: 7px;
      border-radius: 50%;
      background: var(--space-hue);
    }

    @media (forced-colors: active) {
      .swatch[aria-checked="true"],
      .symbols button[aria-checked="true"] {
        outline: 2px solid Highlight;
      }
    }
  }
</style>
