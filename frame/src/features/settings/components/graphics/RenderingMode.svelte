<script lang="ts">
  import * as m from "$shared/i18n/messages";
  import Badge from "$shared/ui/Badge";
  import Button from "$shared/ui/Button";
  import Icon from "$shared/ui/Icon";
  import {
    Alert02Icon,
    CommandLineIcon,
    InformationCircleIcon,
    RefreshIcon,
  } from "@hugeicons/core-free-icons";
  import { graphicsStatus, modeName, modes } from "../../lib/graphics";
  import type { Graphics } from "../../lib/graphics-state.svelte";

  let { graphics }: { graphics: Graphics } = $props();
  const uid = $props.id();
  let group: HTMLDivElement;

  let report = $derived(graphics.report());
  let status = $derived(report ? graphicsStatus(report) : null);
  let ready = $derived(report !== null);
  let chosen = $derived(graphics.chosen());
  // The first option takes focus until a choice is known.
  let focusable = $derived(chosen ?? modes[0].value);

  function key(event: KeyboardEvent) {
    if (!ready) return;
    const keys: Record<string, number> = {
      ArrowDown: 1,
      ArrowRight: 1,
      ArrowUp: -1,
      ArrowLeft: -1,
    };
    let index = modes.findIndex((mode) => mode.value === focusable);
    if (event.key === "Home") index = 0;
    else if (event.key === "End") index = modes.length - 1;
    else if (event.key in keys) {
      const rtl = getComputedStyle(group).direction === "rtl" && event.key.endsWith("t");
      const step = keys[event.key]! * (rtl ? -1 : 1);
      index = (index + step + modes.length) % modes.length;
    } else return;
    event.preventDefault();
    const next = modes[index]!.value;
    group.querySelector<HTMLElement>(`[data-mode="${next}"]`)?.focus();
    void graphics.choose(next);
  }
</script>

<div
  class="modes"
  role="radiogroup"
  aria-label={m.graphics_mode()}
  aria-describedby="{uid}-status"
  aria-disabled={!ready || undefined}
  aria-busy={graphics.saving() || undefined}
  tabindex="-1"
  data-setting="graphics.mode"
  bind:this={group}
  onkeydown={key}
>
  {#each modes as mode (mode.value)}
    {@const checked = chosen === mode.value}
    <button
      type="button"
      role="radio"
      class="mode"
      aria-checked={checked}
      aria-describedby="{uid}-{mode.value}"
      aria-disabled={!ready || undefined}
      tabindex={mode.value === focusable ? 0 : -1}
      data-mode={mode.value}
      onclick={() => ready && void graphics.choose(mode.value)}
    >
      <span class="dot" aria-hidden="true"></span>
      <span class="copy">
        <span class="name"
          >{mode.label()}{#if mode.value === "automatic"}<Badge>{m.graphics_recommended()}</Badge
            >{/if}</span
        >
        <span class="description" id="{uid}-{mode.value}">{mode.description()}</span>
      </span>
    </button>
  {/each}
</div>

<div
  class="status"
  id="{uid}-status"
  data-kind={status?.kind ?? (graphics.failed() ? "failed" : "loading")}
>
  {#if status}
    <span class="glyph" aria-hidden="true"
      ><Icon
        icon={status.kind === "environment"
          ? CommandLineIcon
          : status.kind === "restart"
            ? RefreshIcon
            : status.kind === "recovered"
              ? Alert02Icon
              : InformationCircleIcon}
        size={16}
      /></span
    >
    <div class="text">
      <h3>{status.title}</h3>
      <p>
        {status.kind === "restart" && graphics.restartFailed()
          ? m.navigation_error_restart_failed()
          : status.detail}
      </p>
      {#if status.kind === "environment" && status.variables.length}<ul class="variables">
          {#each status.variables as variable (variable)}<li><code>{variable}</code></li>{/each}
        </ul>{/if}
    </div>
    {#if status.kind === "restart"}
      <Button
        size="compact"
        variant="primary"
        pending={graphics.restarting()}
        onclick={() => void graphics.restart()}>{m.navigation_error_restart()}</Button
      >
    {:else if status.kind === "recovered"}
      <Button
        size="compact"
        disabled={graphics.saving()}
        onclick={() => void graphics.choose(status.keep)}
        >{m.graphics_recovered_keep({ mode: modeName(status.keep) })}</Button
      >
    {/if}
  {:else if graphics.failed()}
    <span class="glyph" aria-hidden="true"><Icon icon={Alert02Icon} size={16} /></span>
    <div class="text">
      <h3>{m.graphics_load_failed_title()}</h3>
      <p>{m.graphics_load_failed()}</p>
    </div>
    <Button size="compact" onclick={graphics.retry}>{m.surface_retry()}</Button>
  {:else}
    <span class="glyph" aria-hidden="true"></span>
    <div class="text" aria-hidden="true">
      <span class="placeholder title"></span><span class="placeholder line"></span>
    </div>
  {/if}
</div>
{#if graphics.saveFailed()}<p class="feedback" role="alert">{m.graphics_save_failed()}</p>{/if}

<style>
  .modes {
    outline: none;
  }

  .mode {
    position: relative;
    display: flex;
    align-items: flex-start;
    gap: 12px;
    box-sizing: border-box;
    inline-size: 100%;
    min-block-size: var(--row-page);
    padding: 13px 18px;
    border: 0;
    background: transparent;
    color: var(--color-text);
    font: inherit;
    text-align: start;
    cursor: default;
    transition: background-color var(--motion-instant) var(--ease-smooth);
  }

  .mode + .mode::before {
    content: "";
    position: absolute;
    inset-inline: 46px 0;
    top: 0;
    height: 1px;
    background: var(--color-border);
  }

  .mode:focus-visible {
    outline: 2px solid var(--color-ring);
    outline-offset: -2px;
  }

  .mode:hover:not([aria-disabled="true"]) {
    background: var(--row-hover);
  }

  /* A radio, drawn as the checkbox is: the control fill when off, the lit
     rung when on, and a dot of the page colour inside it. */
  .dot {
    display: grid;
    flex: none;
    place-items: center;
    box-sizing: border-box;
    inline-size: 16px;
    block-size: 16px;
    margin-block-start: 2px;
    border-radius: 50%;
    background: var(--color-control);
    box-shadow: var(--shadow-control);
    transition:
      background-color var(--motion-instant) var(--ease-smooth),
      box-shadow var(--motion-instant) var(--ease-smooth);
  }

  .dot::after {
    content: "";
    inline-size: 6px;
    block-size: 6px;
    border-radius: 50%;
    background: var(--color-on-lit);
    scale: 0;
    transition: scale var(--motion-slow) var(--ease-spring);
  }

  .mode[aria-checked="true"] .dot {
    background: var(--color-lit);
    box-shadow: none;
  }

  .mode[aria-checked="true"] .dot::after {
    scale: 1;
  }

  .mode[aria-checked="false"]:hover:not([aria-disabled="true"]) .dot {
    background: var(--color-control-hover);
  }

  .copy {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-inline-size: 0;
  }

  .mode[aria-disabled="true"] .copy {
    opacity: 0.6;
  }

  .name {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
    font-size: var(--text-page-title);
    font-weight: 500;
    line-height: 19px;
    letter-spacing: -0.008em;
  }

  .description {
    color: var(--color-muted);
    font-size: var(--text-label);
    line-height: 1.5;
  }

  .status {
    position: relative;
    display: flex;
    align-items: center;
    gap: 12px;
    box-sizing: border-box;
    min-block-size: 68px;
    padding: 14px 18px;
    background: var(--color-fill);
  }

  .status::before {
    content: "";
    position: absolute;
    inset-inline: 0;
    top: 0;
    height: 1px;
    background: var(--color-border);
  }

  /* Set against the title's first line, however long the explanation runs. */
  .glyph {
    display: grid;
    flex: none;
    align-self: flex-start;
    place-items: center;
    inline-size: 16px;
    block-size: 19px;
    color: var(--color-muted);
  }

  .status[data-kind="restart"] .glyph,
  .status[data-kind="environment"] .glyph {
    color: var(--color-text);
  }

  .status[data-kind="recovered"] .glyph,
  .status[data-kind="failed"] .glyph {
    color: var(--color-warning);
  }

  .text {
    flex: 1;
    min-inline-size: 0;
  }

  h3 {
    margin: 0;
    color: var(--color-text);
    font-size: var(--text-page-title);
    font-weight: 500;
    line-height: 19px;
    letter-spacing: -0.008em;
  }

  p {
    margin: 2px 0 0;
    color: var(--color-muted);
    font-size: var(--text-label);
    line-height: 1.5;
  }

  .variables {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin: 8px 0 0;
    padding: 0;
    list-style: none;
  }

  code {
    display: inline-block;
    padding: 2px 6px;
    border-radius: 4px;
    background: var(--color-fill-strong);
    color: var(--color-text);
    font-family: var(--font-mono);
    font-size: var(--text-caption);
    line-height: 16px;
    overflow-wrap: anywhere;
  }

  .placeholder {
    display: block;
    border-radius: 4px;
    background: var(--color-fill-strong);
  }

  .placeholder.title {
    inline-size: 38%;
    block-size: 12px;
    margin-block: 3px 7px;
  }

  .placeholder.line {
    inline-size: 64%;
    block-size: 10px;
  }

  .feedback {
    margin: 0;
    padding: 0 18px 14px;
    background: var(--color-fill);
    color: var(--color-danger);
  }

  @container (max-width: 600px) {
    .status {
      flex-wrap: wrap;
    }

    .text {
      flex-basis: calc(100% - 28px);
    }

    .status :global(.ui-button) {
      margin-inline-start: 28px;
    }
  }

  @media (forced-colors: active) {
    .dot {
      forced-color-adjust: none;
      border: 1px solid ButtonText;
      background: Canvas;
      box-shadow: none;
    }

    .mode[aria-checked="true"] .dot {
      border-color: Highlight;
      background: Highlight;
    }

    .mode[aria-checked="false"]:hover:not([aria-disabled="true"]) .dot {
      background: Canvas;
    }

    .dot::after {
      background: HighlightText;
    }

    .mode:focus-visible {
      outline-color: Highlight;
    }

    .status::before,
    .mode + .mode::before {
      background: CanvasText;
    }

    .placeholder,
    code {
      border: 1px solid CanvasText;
    }
  }
</style>
