<script lang="ts">
  import * as m from "$shared/i18n/messages";
  import Button from "$shared/ui/Button";
  import Icon from "$shared/ui/Icon";
  import { Orb } from "$shared/ui/presence";
  import type { PageAssistDecisionV1, PageAssistStepV1 } from "$shared/ipc/bindings";
  import { consequence, controlOf, labelParts, stepReason } from "../../lib/format";
  import { AlertCircleIcon, Cancel01Icon, Tick02Icon } from "../../lib/icons";
  import "../assist.css";

  let {
    steps,
    site,
    ondecide,
  }: {
    steps: readonly PageAssistStepV1[];
    site: string | null;
    ondecide: (decision: PageAssistDecisionV1) => void;
  } = $props();

  const waiting = $derived(steps.find((step) => step.state === "confirm") ?? null);
  let answered = $state(false);
  $effect(() => {
    void waiting;
    answered = false;
  });

  function decide(decision: PageAssistDecisionV1) {
    answered = true;
    ondecide(decision);
  }
</script>

{#snippet label(text: string)}{#each labelParts(text) as part, index (index)}{#if part.named}<span
        class="named">“{part.text}”</span
      >{:else}{part.text}{/if}{/each}{/snippet}

<ol class="steps assist-list" aria-label={m.assist_steps_label()}>
  {#each steps as step (step.index)}
    {#if step === waiting}
      <li class="confirm assist-rise" aria-label={m.assist_confirm_label()}>
        <h4>{m.assist_confirm_title({ control: controlOf(step.label) })}</h4>
        <p>{consequence(step.category, site)}</p>
        {#if step.category === "type"}
          <p>{m.assist_typing_review()}</p>
          <p class="typing-review">{step.label}</p>
        {/if}
        <div class="assist-actions">
          <Button
            variant="primary"
            size="compact"
            class="control"
            disabled={answered}
            onclick={() => decide("approve")}>{controlOf(step.label)}</Button
          >
          <Button
            variant="ghost"
            size="compact"
            disabled={answered}
            onclick={() => decide("decline")}>{m.assist_decline()}</Button
          >
        </div>
      </li>
    {:else}
      <li class="step" data-state={step.state}>
        <span class="glyph" aria-hidden="true">
          {#if step.state === "running"}<Orb size={13} />{:else if step.state === "done"}<Icon
              icon={Tick02Icon}
              size={14}
              strokeWidth={2.1}
            />{:else if step.state === "failed"}<Icon
              icon={AlertCircleIcon}
              size={14}
            />{:else}<Icon icon={Cancel01Icon} size={11} strokeWidth={2.2} />{/if}
        </span>
        <span class="words">
          <span class="label">{@render label(step.label)}</span>
          {#if step.state === "failed"}<span class="why">{stepReason(step.reason)}</span
            >{:else if step.state === "declined"}<span class="why">{m.assist_step_declined()}</span
            >{:else if step.state === "skipped"}<span class="why">{m.assist_step_skipped()}</span
            >{/if}
        </span>
      </li>
    {/if}
  {/each}
</ol>

<style>
  .step {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    min-block-size: 36px;
    box-sizing: border-box;
    padding: 9px 10px;
    color: var(--color-label-secondary);
    font-size: 13px;
    line-height: 18px;
  }

  .glyph {
    display: grid;
    flex: none;
    place-items: center;
    inline-size: 16px;
    block-size: 18px;
    color: var(--color-faint);
  }

  .step[data-state="done"] .glyph {
    color: var(--color-text);
  }

  .step[data-state="failed"] .glyph {
    color: var(--color-warning);
  }

  .words {
    display: flex;
    flex-direction: column;
    gap: 1px;
    min-inline-size: 0;
  }

  .label {
    overflow-wrap: anywhere;
  }

  .named {
    color: var(--color-text);
    font-weight: 500;
  }

  .step:is([data-state="declined"], [data-state="skipped"]) :is(.label, .named) {
    color: var(--color-faint);
  }

  .why {
    color: var(--color-muted);
    font-size: 12.5px;
    line-height: 17px;
    text-wrap: pretty;
  }

  .step:is([data-state="declined"], [data-state="skipped"]) .why {
    color: var(--color-faint);
  }

  /* The step that waits opens in place: the same row, lifted, with what it does and the two answers. */
  .confirm {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin-block: 2px;
    padding: 12px 12px 12px 36px;
    border-radius: var(--radius-control);
    background: var(--row-active);
    box-shadow: var(--shine-top);
  }

  .confirm h4 {
    margin: 0;
    color: var(--color-text);
    font-size: var(--text-page-title);
    font-weight: 600;
    letter-spacing: -0.01em;
    line-height: 19px;
    text-wrap: pretty;
  }

  .confirm p {
    margin: 0;
    color: var(--color-muted);
    font-size: 13px;
    line-height: 18px;
  }

  .confirm .assist-actions {
    margin-block-start: 6px;
  }

  .typing-review {
    overflow-wrap: anywhere;
    white-space: pre-wrap;
  }

  .confirm :global(.control) {
    max-inline-size: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
