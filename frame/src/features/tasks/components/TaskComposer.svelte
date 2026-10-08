<script lang="ts">
  import { duration, easing, reducedMotion } from "$shared/lib/motion";
  import Icon from "$shared/ui/Icon";
  import { fitTextarea, scrollParent } from "$shared/lib/fit";
  import Menu, { type MenuEntry } from "$shared/ui/Menu";
  import {
    Add01Icon,
    ArrowUp02Icon,
    Calendar03Icon,
    Cancel01Icon,
    Clock01Icon,
    Folder01Icon,
    InboxIcon,
    Link01Icon,
  } from "@hugeicons/core-free-icons";
  import type { TaskContext, TaskList, TaskPriority } from "$domain/resources";
  import * as m from "$shared/i18n/messages";
  import { dueLabel, durationLabel, hostOf } from "../lib/task-sections";
  import { parseCapture, parseTokens, type TokenKind } from "../lib/task-language";
  import { PRIORITY_ICON } from "../lib/priority";
  import { DUE_LABELS, DURATION_LABELS, PRIORITY_LABEL } from "../lib/labels";
  import DueMenu from "./DueMenu.svelte";
  import SiteMark from "./SiteMark.svelte";

  type Refusable = TokenKind | "date";

  let {
    today,
    value = $bindable(""),
    context = $bindable<TaskContext | null>(null),
    page = null,
    lists = [],
    initialList = null,
    defaultDate = null,
    density = "panel",
    compact = false,
    oncreate,
    onexit,
  }: {
    today: string;
    value?: string;
    context?: TaskContext | null;
    /** The page in front of the reader, which a capture can carry along. */
    page?: TaskContext | null;
    lists?: readonly TaskList[];
    initialList?: string | null;
    defaultDate?: string | null;
    density?: "panel" | "rail" | "page";
    /** Properties as bare marks, for the narrow sidebar column; elsewhere they
     *  carry their names. */
    compact?: boolean;
    oncreate: (input: {
      title: string;
      dueDate: string | null;
      dueTime: string | null;
      duration: number | null;
      list: string | null;
      inbox: boolean;
      priority: TaskPriority;
      context: TaskContext | null;
      /** The field as it read when submitted. */
      draft: string;
    }) => Promise<string | null>;
    onexit?: () => void;
  } = $props();

  let field = $state<HTMLTextAreaElement>();
  let root = $state<HTMLElement>();
  let focused = $state(false);
  let refused = $state<Partial<Record<Refusable, string>>>({});
  let list = $derived(initialList ?? "inbox");
  let date = $derived(defaultDate);
  let time = $state<string | null>(null);
  let priority = $state<TaskPriority>("none");
  const uid = $props.id();

  // Closing follows where the reader went next, never a press in progress:
  // collapsing mid-press moves the list, so the release would land on another
  // row. What decides is where the press began, because a picker item removes
  // itself before its click finishes. The keyboard closes it when focus moves.
  let pressing = false;
  let pressedInside = false;
  const inside = (target: EventTarget | null) =>
    target instanceof Element && (root?.contains(target) || target.closest(".ui-menu") !== null);
  $effect(() => {
    if (!focused) return;
    const press = (event: PointerEvent) => {
      pressing = true;
      pressedInside = inside(event.target);
    };
    const release = () => {
      // The click that focused the composer began before this listened.
      if (!pressing) return;
      pressing = false;
      if (!pressedInside) focused = false;
    };
    const cancel = () => (pressing = false);
    document.addEventListener("pointerdown", press, true);
    document.addEventListener("click", release, true);
    document.addEventListener("pointercancel", cancel, true);
    return () => {
      document.removeEventListener("pointerdown", press, true);
      document.removeEventListener("click", release, true);
      document.removeEventListener("pointercancel", cancel, true);
    };
  });

  function leave(event: FocusEvent) {
    if (pressing || event.relatedTarget === null || inside(event.relatedTarget)) return;
    focused = false;
  }

  export function focus() {
    field?.focus();
  }

  // A refused reading stays refused only while its words are still typed.
  let skip = $derived(
    new Set(
      (Object.entries(refused) as [Refusable, string][])
        .filter(([kind, text]) => kind !== "date" && value.includes(text))
        .map(([kind]) => kind as TokenKind),
    ),
  );
  let tokens = $derived(parseTokens(value, lists, skip));
  let read = $derived(parseCapture(tokens.rest, today));
  let reading = $derived(read.matched !== null && read.matched !== refused.date ? read : null);

  let title = $derived(reading ? reading.title : tokens.rest);
  let dueDate = $derived(reading?.dueDate ?? date);
  let dueTime = $derived(reading ? reading.dueTime : time);
  let chosenPriority = $derived(tokens.priority ?? priority);
  let chosenList = $derived(tokens.list ?? list);
  let open = $derived(focused || value.length > 0 || context !== null);

  let listEntries: MenuEntry[] = $derived([
    {
      kind: "item",
      id: "inbox",
      label: m.task_scope_inbox(),
      icon: InboxIcon,
      checked: chosenList === "inbox",
    },
    { kind: "item", id: "none", label: m.task_no_list(), checked: chosenList === "none" },
    ...(lists.length ? [{ kind: "separator" as const }] : []),
    ...lists.map((entry) => ({
      kind: "item" as const,
      id: entry.id,
      label: entry.title,
      icon: Folder01Icon,
      checked: chosenList === entry.id,
    })),
  ]);
  let priorityEntries: MenuEntry[] = $derived(
    (["high", "medium", "low", "none"] as const).map((id) => ({
      kind: "item",
      id,
      label: PRIORITY_LABEL[id](),
      icon: PRIORITY_ICON[id],
      checked: chosenPriority === id,
    })),
  );
  let listLabel = $derived(
    chosenList === "inbox"
      ? m.task_scope_inbox()
      : chosenList === "none"
        ? m.task_no_list()
        : (lists.find((entry) => entry.id === chosenList)?.title ?? m.task_scope_inbox()),
  );

  function refuse(kind: Refusable) {
    const text = kind === "date" ? reading?.matched : tokens.matched[kind];
    if (text) refused = { ...refused, [kind]: text };
  }

  function grow(node: HTMLTextAreaElement, _value: string) {
    let scroller: HTMLElement | null | undefined;
    const size = () => fitTextarea(node, (scroller ??= scrollParent(node)));
    size();
    return { update: size };
  }

  /** Sends the line and empties the field in the same moment, so the next
   *  task can be typed while this one is saved and a second press finds
   *  nothing to send again. The field is never disabled: that would drop
   *  focus and the keys typed meanwhile. */
  async function submit() {
    const name = title.trim();
    if (!name) return;
    const sent = { value, context, refused, priority, time };
    const saving = oncreate({
      title: name,
      dueDate,
      dueTime: dueDate ? dueTime : null,
      duration: tokens.duration,
      list: chosenList === "inbox" || chosenList === "none" ? null : chosenList,
      inbox: chosenList === "inbox",
      priority: chosenPriority,
      context,
      draft: value,
    });
    value = "";
    context = null;
    refused = {};
    priority = "none";
    time = null;
    field?.focus();
    const id = await saving.catch(() => null);
    // Not saved: the words come back, unless new ones have been typed since.
    if (!id && value === "" && context === null)
      ({ value, context, refused, priority, time } = sent);
  }

  function keydown(event: KeyboardEvent) {
    if (event.isComposing) return;
    if (event.key === "Enter") {
      event.preventDefault();
      void submit();
    }
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      if (reading) refuse("date");
      else {
        focused = false;
        field?.blur();
        onexit?.();
      }
    }
  }

  /** The whole composer is the target, not only the text inside it. */
  function reach(event: MouseEvent) {
    if ((event.target as Element).closest("button, textarea, input, a")) return;
    field?.focus();
  }

  // Its properties open out of the line being written rather than appearing
  // below it, so the list underneath is pushed down, not jumped.
  function unfold(node: HTMLElement) {
    if (reducedMotion()) return;
    const height = node.scrollHeight;
    node.style.overflow = "hidden";
    const opening = node.animate(
      [
        { height: "0px", opacity: 0 },
        { height: `${height}px`, opacity: 1 },
      ],
      { duration: duration("base"), easing: easing("emphasized") },
    );
    void opening.finished.catch(() => undefined).then(() => (node.style.overflow = ""));
  }
</script>

{#snippet refusal(kind: Refusable, text: string)}<button
    type="button"
    class="capture-refuse"
    aria-label={m.task_capture_keep_words({ text })}
    title={m.task_capture_keep_words({ text })}
    onclick={() => refuse(kind)}><Icon icon={Cancel01Icon} size={11} /></button
  >{/snippet}

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div
  bind:this={root}
  class="capture"
  data-open={open}
  data-density={density}
  onclick={reach}
  onfocusin={() => (focused = true)}
  onfocusout={leave}
>
  <div class="capture-line">
    <!-- The plus marks an empty place to write; once writing starts it gives
         the words its room. -->
    {#if !open}<span class="capture-plus" aria-hidden="true"
        ><Icon icon={Add01Icon} size={16} /></span
      >{/if}<textarea
      bind:this={field}
      use:grow={value}
      class="capture-title"
      rows="1"
      aria-label={m.tool_add_task()}
      aria-describedby={reading ? uid : undefined}
      placeholder={m.task_new_placeholder()}
      maxlength="256"
      {value}
      oninput={(event) => (value = event.currentTarget.value.replace(/\n/gu, " "))}
      onkeydown={keydown}></textarea>
  </div>
  {#if open}<div class="capture-properties" {@attach unfold}>
      <div class="capture-chips">
        <span class="capture-chip" data-parsed={reading !== null}>
          <DueMenu
            due={dueDate}
            time={dueTime}
            {today}
            label={m.task_date()}
            onselect={(day, clock) => {
              refuse("date");
              date = day;
              time = clock;
            }}
            >{#snippet trigger({ props })}<button
                {...props}
                id={reading ? uid : undefined}
                class="capture-property"
                type="button"
                data-empty={dueDate === null}
                aria-label={dueDate
                  ? `${m.task_date()}: ${dueLabel(dueDate, today, DUE_LABELS, dueTime)}`
                  : m.task_date()}
                title={m.task_date()}
                ><Icon icon={Calendar03Icon} size={14} />{#if dueDate}{dueLabel(
                    dueDate,
                    today,
                    DUE_LABELS,
                    dueTime,
                  )}{:else if !compact}{m.task_date()}{/if}</button
              >{/snippet}</DueMenu
          >{#if reading?.matched}{@render refusal("date", reading.matched)}{/if}
        </span>
        <span class="capture-chip" data-parsed={tokens.list !== null}>
          <Menu
            label={m.task_organization()}
            entries={listEntries}
            triggerClass="capture-property"
            onselect={(id) => {
              refuse("list");
              list = id;
            }}
            >{#snippet trigger()}<span class="capture-value"
                >{#if !compact}<Icon
                    icon={chosenList === "inbox" ? InboxIcon : Folder01Icon}
                    size={14}
                  />{/if}{listLabel}</span
              >{/snippet}</Menu
          >{#if tokens.matched.list}{@render refusal("list", tokens.matched.list)}{/if}
        </span>
        <span class="capture-chip" data-parsed={tokens.priority !== null}>
          <Menu
            label={`${m.task_priority()}: ${PRIORITY_LABEL[chosenPriority]()}`}
            entries={priorityEntries}
            triggerClass="capture-property"
            onselect={(id) => {
              refuse("priority");
              priority = id as TaskPriority;
            }}
            >{#snippet trigger()}<span class="capture-value" data-empty={chosenPriority === "none"}
                ><Icon
                  icon={PRIORITY_ICON[chosenPriority]}
                  size={14}
                />{#if !compact}{chosenPriority === "none"
                    ? m.task_priority()
                    : PRIORITY_LABEL[chosenPriority]()}{/if}</span
              >{/snippet}</Menu
          >{#if tokens.matched.priority}{@render refusal("priority", tokens.matched.priority)}{/if}
        </span>
        {#if tokens.duration !== null}<span class="capture-chip" data-parsed="true"
            ><span class="capture-property"
              ><Icon icon={Clock01Icon} size={14} />{durationLabel(
                tokens.duration,
                DURATION_LABELS,
              )}</span
            >{@render refusal("duration", tokens.matched.duration!)}</span
          >{/if}
        {#if context}<span class="capture-chip capture-page" data-parsed="true"
            ><span class="capture-property" title={context.url}
              ><SiteMark url={context.url} size={14} /><span>{hostOf(context.url)}</span></span
            ><button
              type="button"
              class="capture-refuse"
              aria-label={m.task_capture_remove_context()}
              title={m.task_capture_remove_context()}
              onclick={() => (context = null)}><Icon icon={Cancel01Icon} size={11} /></button
            ></span
          >{:else if page}<span class="capture-chip"
            ><button
              type="button"
              class="capture-property"
              aria-label={m.task_attach_page()}
              title={m.task_attach_page()}
              onclick={() => (context = page)}
              ><Icon icon={Link01Icon} size={14} />{#if !compact}{m.task_attach_page()}{/if}</button
            ></span
          >{/if}
      </div>
      <!-- Beside the properties rather than among them: they wrap within their
           own space, and the action keeps its place at the end of the first row. -->
      <button
        type="button"
        class="capture-submit"
        aria-label={m.task_add_action()}
        title={m.task_add_action()}
        disabled={!title.trim()}
        onclick={() => void submit()}><Icon icon={ArrowUp02Icon} size={14} /></button
      >
    </div>{/if}
</div>

<style>
  .capture {
    flex: none;
    margin-block: 2px 10px;
    padding: 0 6px 0 10px;
    border-radius: var(--radius-row);
    cursor: text;
    transition: background-color var(--motion-fast) var(--ease-out);
  }

  .capture:hover:not([data-open="true"]) {
    background: var(--row-hover);
  }

  .capture[data-open="true"] {
    padding-block-end: 8px;
    background: var(--color-fill);
  }

  .capture-line {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    padding-block: 9px;
  }

  .capture[data-density="page"] .capture-line {
    gap: 12px;
    padding-block: 11px;
  }

  .capture-plus {
    display: grid;
    place-items: center;
    flex: none;
    width: 18px;
    height: 20px;
    color: var(--color-muted);
  }

  .capture-title {
    display: block;
    flex: 1;
    min-width: 0;
    height: 20px;
    padding: 0;
    overflow: hidden;
    border: 0;
    background: transparent;
    color: var(--color-text);
    font: inherit;
    font-size: 14px;
    line-height: 20px;
    resize: none;
    outline: none;
  }

  .capture-title::placeholder {
    color: var(--color-muted);
  }

  .capture-properties {
    display: flex;
    align-items: flex-start;
    gap: 8px;
    cursor: default;
  }

  .capture-chips {
    display: flex;
    flex: 1;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    min-width: 0;
  }

  .capture-chip {
    display: inline-flex;
    align-items: center;
    min-width: 0;
    border-radius: var(--radius-inset);
    box-shadow: inset 0 0 0 1px var(--color-border);
  }

  /* A value read out of the typed line is lit, so the reader sees what the
     words were taken to mean before the task exists. */
  .capture-chip[data-parsed="true"] {
    background: var(--color-accent-soft);
    box-shadow: none;
  }

  .capture :global(.capture-property) {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 5px;
    max-width: 180px;
    min-width: 26px;
    height: 26px;
    padding: 0 7px;
    border: 0;
    border-radius: var(--radius-inset);
    background: transparent;
    color: var(--color-text);
    font: inherit;
    font-size: var(--text-label);
    white-space: nowrap;
    cursor: default;
    outline: none;
    transition: background-color var(--motion-instant) var(--ease-smooth);
  }

  .capture :global(.capture-property span) {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .capture :global(.capture-property:hover:not(:disabled)),
  .capture :global(.capture-property[data-state="open"]) {
    background: var(--row-hover);
  }

  .capture :global(.capture-property:focus-visible) {
    outline: 2px solid var(--color-ring);
    outline-offset: -2px;
  }

  .capture :global([data-empty="true"]) {
    color: var(--color-muted);
  }

  .capture-refuse {
    display: grid;
    place-items: center;
    width: 20px;
    height: 26px;
    margin-inline-start: -4px;
    padding: 0;
    border: 0;
    border-radius: var(--radius-inset);
    background: transparent;
    color: var(--color-muted);
    cursor: default;
    outline: none;
  }

  .capture-refuse:hover {
    color: var(--color-text);
  }

  .capture-refuse:focus-visible {
    outline: 2px solid var(--color-ring);
    outline-offset: -2px;
  }

  /* Rides the line being written, top-aligned with its first row, so the chips
     below can wrap as they need without pushing the action anywhere. */
  .capture-submit {
    display: grid;
    place-items: center;
    flex: none;
    width: 26px;
    height: 26px;
    padding: 0;
    border: 0;
    border-radius: var(--radius-capsule);
    background: var(--color-lit);
    color: var(--color-on-lit);
    cursor: default;
    outline: none;
    transition: background-color var(--motion-instant) var(--ease-smooth);
  }

  .capture-submit:focus-visible {
    outline: 2px solid var(--color-ring);
    outline-offset: 2px;
  }

  /* The chrome around it is not selectable; the words being written are. */
  .capture-title,
  textarea {
    /* stylelint-disable-next-line property-no-vendor-prefix */
    -webkit-user-select: text;
    user-select: text;
  }

  .capture-submit:disabled {
    background: var(--color-fill-strong);
    color: var(--color-faint);
  }

  .capture-submit:hover:not(:disabled) {
    background: var(--color-lit-hover);
  }
</style>
