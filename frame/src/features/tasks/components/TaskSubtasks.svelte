<script lang="ts">
  import { tick } from "svelte";
  import type { TaskStep } from "$domain/resources";
  import Icon from "$shared/ui/Icon";
  import { Add01Icon, Cancel01Icon } from "@hugeicons/core-free-icons";
  import * as m from "$shared/i18n/messages";
  import TaskCheck from "./TaskCheck.svelte";

  let {
    steps,
    disabled = false,
    onchange,
    onrename,
    onsettle,
  }: {
    steps: readonly TaskStep[];
    disabled?: boolean;
    onchange: (steps: TaskStep[]) => Promise<boolean>;
    onrename: (id: string, title: string) => void;
    /** The reader left a step's title. */
    onsettle?: () => void;
  } = $props();
  let value = $state("");
  let field = $state<HTMLInputElement>();
  let busy = $state(false);
  let done = $derived(steps.filter((step) => step.completed).length);

  async function add() {
    if (!value.trim() || busy) return;
    busy = true;
    const step = { id: crypto.randomUUID(), title: value.trim().slice(0, 256), completed: false };
    const saved = await onchange([...steps, step]);
    if (saved || steps.some((entry) => entry.id === step.id)) value = "";
    busy = false;
    await tick();
    field?.focus();
  }

  function toggle(step: TaskStep) {
    void onchange(
      steps.map((entry) =>
        entry.id === step.id ? { ...entry, completed: !entry.completed } : entry,
      ),
    );
  }
</script>

<section class="subtasks" aria-label={m.task_subtasks()}>
  {#if steps.length}<header>
      <span>{m.task_subtasks()}</span><small
        aria-label={m.task_subtask_progress({ done, total: steps.length })}
        >{done}/{steps.length}</small
      >
    </header>{/if}
  {#each steps as step (step.id)}<div class="subtask" data-completed={step.completed}>
      <TaskCheck
        size="small"
        status={step.completed ? "done" : "open"}
        label={step.title}
        tabindex={0}
        {disabled}
        ontoggle={() => toggle(step)}
      />
      <input
        type="text"
        class="subtask-title"
        aria-label={m.task_subtask_title()}
        aria-invalid={!step.title.trim() || undefined}
        {disabled}
        maxlength="256"
        value={step.title}
        oninput={(event) => onrename(step.id, event.currentTarget.value)}
        onblur={() => onsettle?.()}
        onkeydown={(event) => {
          if (event.key === "Enter") {
            event.preventDefault();
            field?.focus();
          }
        }}
      />
      {#if !disabled}<button
          type="button"
          class="subtask-remove"
          aria-label={m.task_remove_subtask()}
          title={m.task_remove_subtask()}
          onclick={() => void onchange(steps.filter((entry) => entry.id !== step.id))}
          ><Icon icon={Cancel01Icon} size={13} /></button
        >{/if}
    </div>{/each}
  {#if !disabled && steps.length < 100}<form
      class="subtask subtask-add"
      onsubmit={(event) => {
        event.preventDefault();
        void add();
      }}
    >
      <span class="subtask-plus" aria-hidden="true"><Icon icon={Add01Icon} size={14} /></span><input
        bind:this={field}
        class="subtask-title"
        aria-label={m.task_add_subtask()}
        placeholder={m.task_add_subtask()}
        maxlength="256"
        bind:value
        disabled={busy}
      />
    </form>{/if}
</section>

<style>
  .subtasks {
    display: flex;
    flex-direction: column;
  }

  header {
    display: flex;
    align-items: baseline;
    gap: 8px;
    padding-block-end: 4px;
    color: var(--color-muted);
    font-size: var(--text-label);
    font-weight: 600;
  }

  small {
    color: var(--color-faint);
    font-size: var(--text-caption);
    font-weight: 500;
    font-variant-numeric: tabular-nums;
  }

  .subtask {
    display: flex;
    align-items: center;
    gap: 10px;
    min-height: 32px;
    margin-inline: -8px;
    padding-inline: 8px 4px;
    border-radius: var(--radius-row);
    transition: background-color var(--motion-fast) var(--ease-out);
  }

  .subtask:not(.subtask-add):hover {
    background: var(--row-hover);
  }

  .subtask-plus {
    display: grid;
    place-items: center;
    flex: none;
    width: 15px;
    color: var(--color-faint);
  }

  .subtask-title {
    flex: 1;
    min-width: 0;
    height: 28px;
    padding: 0;
    border: 0;
    background: transparent;
    color: var(--color-text);
    font: inherit;
    font-size: var(--text-body);
    outline: none;
    /* stylelint-disable-next-line property-no-vendor-prefix */
    -webkit-user-select: text;
    user-select: text;
  }

  .subtask-title::placeholder {
    color: var(--color-faint);
  }

  .subtask[data-completed="true"] .subtask-title {
    color: var(--color-faint);
    text-decoration: line-through;
  }

  .subtask-remove {
    display: grid;
    place-items: center;
    flex: none;
    width: 22px;
    height: 22px;
    padding: 0;
    border: 0;
    border-radius: var(--radius-inset);
    background: transparent;
    color: var(--color-faint);
    opacity: 0;
    cursor: default;
    transition: opacity var(--motion-fast) var(--ease-out);
  }

  .subtask-remove:hover {
    background: var(--row-pressed);
    color: var(--color-text);
  }

  .subtask-remove:focus-visible {
    outline: 2px solid var(--color-ring);
    outline-offset: -2px;
  }

  .subtask:hover .subtask-remove,
  .subtask:focus-within .subtask-remove {
    opacity: 1;
  }
</style>
