<script lang="ts">
  import { onMount, tick } from "svelte";
  import * as m from "$shared/i18n/messages";
  import { tabs } from "$domain/tabs";
  import { favicons } from "$domain/favicons";
  import { loadModelPicker, loadVoice } from "$features/work";
  import FavIcon from "$shared/ui/FavIcon";
  import Icon from "$shared/ui/Icon";
  import { MESSAGE_LIMIT, type AssistSession } from "../lib/assist-session.svelte";
  import { faultView } from "../lib/faults";
  import {
    ArrowUp02Icon,
    Cancel01Icon,
    Globe02Icon,
    Mic01Icon,
    QuoteDownIcon,
    StopIcon,
    Target02Icon,
  } from "../lib/icons";
  import AddMenu from "./AddMenu.svelte";

  let {
    session,
    draft,
    ondraft,
  }: {
    session: AssistSession;
    draft: string;
    ondraft: (text: string) => void;
  } = $props();

  let field = $state<HTMLTextAreaElement>();
  let dictating = $state(false);
  let voice = $state<{ toggle(): void }>();

  const status = $derived(session.status);
  const page = $derived(session.pageAttached ? (status?.page ?? null) : null);
  const live = $derived(new Map(tabs.tabs().map((tab) => [tab.id, tab])));
  const markOf = (id: string) => {
    const tab = live.get(id);
    return tab ? favicons.mark(tab.icon, tab.url) : null;
  };
  const others = $derived(
    session.others.flatMap((id) => {
      const tab = live.get(id);
      if (!tab?.url) return [];
      let host = tab.url;
      try {
        host = new URL(tab.url).host.replace(/^www\./u, "");
      } catch {
        // An address that is not a URL stays as it is.
      }
      return [{ id, host, title: tab.title }];
    }),
  );
  const selection = $derived(status?.selection ?? null);
  /** Rust said this conversation cannot talk; until it answers, the field is open to typing. */
  const off = $derived(!!status && !session.talks);
  const busy = $derived(session.busy);
  const ready = $derived(!!draft.trim() && session.talks && !busy && !session.sending);
  const picking = $derived(session.picking !== null);
  const notice = $derived(session.notice ? faultView(session.notice).message : null);
  const chips = $derived(
    !!page || others.length > 0 || selection !== null || session.picks.length > 0 || picking,
  );
  /** One round button: the mic while the field is empty, Send once it has words, Stop while a reply runs. */
  const action = $derived(busy ? "stop" : draft.trim() ? "send" : "mic");

  // Opening the panel, from ⌘E or the dock, lands the caret in the field.
  onMount(() => field?.focus());

  /** Moves the caret into the field; the host calls it when the panel is asked for again. */
  export function focus() {
    field?.focus();
  }

  function grow() {
    if (!field) return;
    field.style.blockSize = "auto";
    field.style.blockSize = `${Math.min(field.scrollHeight, 168)}px`;
  }
  $effect(() => {
    void draft;
    void tick().then(grow);
  });

  async function send() {
    if (!ready) return;
    const sent = draft;
    // Words typed while the question was on its way stay in the field.
    if ((await session.ask(sent)) && draft === sent) ondraft("");
  }

  function onkeydown(event: KeyboardEvent) {
    if (event.key === "Escape" && (busy || picking)) {
      // Escape stops what is happening before it would close the panel.
      event.preventDefault();
      if (picking) session.cancelPick();
      else session.stop();
      return;
    }
    if (event.key === "ArrowUp" && !draft && !event.shiftKey) {
      // From an empty field, up goes to the starts above it.
      const starts = field?.closest("[data-assist]")?.querySelectorAll<HTMLElement>("[data-start]");
      const last = starts?.[starts.length - 1];
      if (last) {
        event.preventDefault();
        last.focus();
      }
      return;
    }
    if (event.key === "Enter" && !event.shiftKey && !event.isComposing) {
      event.preventDefault();
      void send();
    }
  }

  function commit(text: string) {
    const before = draft.trimEnd();
    ondraft(before ? `${before} ${text}` : text);
    dictating = false;
    requestAnimationFrame(() => {
      field?.focus();
      field?.setSelectionRange(field.value.length, field.value.length);
    });
  }

  const pick = () => void session.pick(tabs.activeTab()?.url ?? null);
</script>

<svelte:window
  onkeydown={(event) => {
    if (event.key !== "Escape" || !picking || event.defaultPrevented) return;
    event.preventDefault();
    session.cancelPick();
  }}
/>

{#snippet drop(name: string, ondrop: () => void)}<button
    type="button"
    class="drop"
    aria-label={m.assist_detach({ name })}
    onclick={ondrop}><Icon icon={Cancel01Icon} size={9} strokeWidth={2.6} /></button
  >{/snippet}

<form
  class="surface composer"
  data-off={off || undefined}
  onsubmit={(event) => {
    event.preventDefault();
    void send();
  }}
>
  {#if chips}
    <ul class="chips" aria-label={m.assist_attached_label()}>
      {#if page}
        {@const mark = markOf(page.tab)}
        <li class="chip" title={page.title}>
          <FavIcon image={mark?.image ?? null} tone={mark?.tone} size={14} fallback={Globe02Icon} />
          <span class="name">{page.host}</span>
          {@render drop(page.host, () => (session.pageOff = true))}
        </li>
      {/if}
      {#each others as other (other.id)}
        {@const mark = markOf(other.id)}
        <li class="chip" title={other.title}>
          <FavIcon image={mark?.image ?? null} tone={mark?.tone} size={14} fallback={Globe02Icon} />
          <span class="name">{other.host}</span>
          {@render drop(other.host, () => session.detachTab(other.id))}
        </li>
      {/each}
      {#if selection !== null}
        <li>
          <button
            type="button"
            class="chip toggle"
            aria-pressed={session.selectionOn}
            title={selection}
            onclick={() => (session.selectionOn = !session.selectionOn)}
          >
            <Icon icon={QuoteDownIcon} size={12} strokeWidth={1.8} /><span class="name"
              >{m.assist_selection()}</span
            >
          </button>
        </li>
      {/if}
      {#each session.picks as picked (picked.pick)}
        <li class="chip" title={`${picked.label} · ${picked.host}`}>
          <Icon icon={Target02Icon} size={12} strokeWidth={1.8} />
          <span class="name">{picked.label}</span>
          {@render drop(picked.label, () => session.detach(picked))}
        </li>
      {/each}
      {#if picking}
        <li class="chip hint" role="status">
          <Icon icon={Target02Icon} size={12} strokeWidth={1.8} />
          <span class="name">{m.assist_picking()}</span>
          <button
            type="button"
            class="drop"
            aria-label={m.assist_pick_cancel()}
            onclick={() => session.cancelPick()}
            ><Icon icon={Cancel01Icon} size={9} strokeWidth={2.6} /></button
          >
        </li>
      {/if}
    </ul>
  {/if}
  {#if dictating}
    <div class="voice">
      {#await loadVoice() then view}<view.default
          bind:this={voice}
          oncommit={commit}
          onclose={() => (dictating = false)}
        />{/await}
    </div>
  {:else}
    <textarea
      bind:this={field}
      aria-label={m.assist_placeholder()}
      placeholder={m.assist_placeholder()}
      rows="1"
      maxlength={MESSAGE_LIMIT}
      disabled={off}
      value={draft}
      onfocus={() => session.refresh()}
      oninput={(event) => {
        session.notice = null;
        ondraft(event.currentTarget.value);
      }}
      {onkeydown}></textarea>
  {/if}
  {#if notice}<p class="notice" role="alert">{notice}</p>{/if}
  <div class="bar">
    <AddMenu {session} onpick={pick} />
    <span class="grow"></span>
    <span class="model">
      {#await loadModelPicker() then picker}<picker.default
          profile={session.profile}
          role="page"
        />{/await}
    </span>
    <div class="action" data-action={action}>
      <button
        type="button"
        class="round mic"
        aria-label={m.voice_start()}
        title={m.voice_start()}
        aria-pressed={dictating}
        disabled={off}
        inert={action !== "mic"}
        onclick={() => (dictating ? voice?.toggle() : (dictating = true))}
        ><Icon icon={Mic01Icon} size={16} strokeWidth={1.7} /></button
      >
      <button
        type="submit"
        class="round send"
        aria-label={m.assist_send()}
        title={m.assist_send()}
        disabled={!ready}
        inert={action !== "send"}><Icon icon={ArrowUp02Icon} size={15} strokeWidth={2.2} /></button
      >
      <button
        type="button"
        class="round send stop"
        aria-label={m.assist_stop()}
        title={m.assist_stop()}
        inert={action !== "stop"}
        onclick={() => session.stop()}><Icon icon={StopIcon} size={11} /></button
      >
    </div>
  </div>
</form>

<style>
  /* One soft plate: the lifted material of the sidebar's own field, rounded
     like a sheet, with a hairline and a breath of shadow so it stands on the
     column rather than in it. */
  .composer {
    display: flex;
    flex: none;
    flex-direction: column;
    margin: 4px 10px 10px;
    padding: 4px;
    border-radius: var(--radius-card);
    background: var(--row-active);
    box-shadow: var(--shine-top), var(--shadow-raised);
    transition: box-shadow var(--motion-fast) var(--ease-out);
  }

  .composer:focus-within {
    box-shadow: var(--shine-top), var(--shadow-raised), var(--shadow-field-focus);
  }

  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    margin: 0;
    padding: 4px 4px 0;
    list-style: none;
  }

  .chip {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    box-sizing: border-box;
    max-inline-size: 100%;
    block-size: 24px;
    padding: 0 3px 0 7px;
    border: 0;
    border-radius: var(--radius-inset);
    background: var(--color-fill-active);
    color: var(--color-label-secondary);
    font: inherit;
    font-size: 12px;
    font-weight: 500;
    letter-spacing: -0.002em;
  }

  .name {
    min-inline-size: 0;
    max-inline-size: 168px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .hint {
    padding-inline-start: 8px;
    background: transparent;
    box-shadow: inset 0 0 0 1px var(--color-border-strong);
    color: var(--color-muted);
  }

  .hint .name {
    max-inline-size: none;
  }

  .toggle {
    padding-inline-end: 9px;
    color: var(--color-muted);
    cursor: default;
    transition:
      background-color var(--motion-instant) var(--ease-smooth),
      color var(--motion-instant) var(--ease-smooth);
  }

  .toggle:hover {
    color: var(--color-text);
  }

  .toggle[aria-pressed="true"] {
    background: var(--color-lit);
    color: var(--color-on-lit);
  }

  .drop {
    display: grid;
    flex: none;
    place-items: center;
    inline-size: 18px;
    block-size: 18px;
    padding: 0;
    border: 0;
    border-radius: var(--radius-capsule);
    background: transparent;
    color: var(--color-faint);
    cursor: default;
    transition:
      background-color var(--motion-instant) var(--ease-smooth),
      color var(--motion-instant) var(--ease-smooth);
  }

  .drop:hover {
    background: var(--color-fill-hover);
    color: var(--color-text);
  }

  .toggle:focus-visible,
  .drop:focus-visible {
    outline: 2px solid var(--color-ring);
  }

  textarea {
    inline-size: 100%;
    min-block-size: 40px;
    max-block-size: 168px;
    box-sizing: border-box;
    padding: 10px 10px 8px;
    border: 0;
    outline: none;
    background: transparent;
    color: var(--color-text);
    font: inherit;
    font-size: var(--text-page-title);
    letter-spacing: -0.004em;
    line-height: 22px;
    resize: none;
  }

  textarea::placeholder {
    color: var(--color-faint);
  }

  .composer[data-off] textarea {
    opacity: 0.5;
  }

  .voice {
    min-block-size: 40px;
    padding: 4px 4px 0;
  }

  .notice {
    margin: 0;
    padding: 0 10px 4px;
    color: var(--color-warning);
    font-size: 12.5px;
    line-height: 1.4;
  }

  .bar {
    display: flex;
    align-items: center;
    gap: 2px;
    min-inline-size: 0;
    block-size: 32px;
  }

  .grow {
    flex: 1;
  }

  .bar :global(.tool) {
    display: grid;
    flex: none;
    place-items: center;
    inline-size: 28px;
    block-size: 28px;
    margin-inline-start: 2px;
    padding: 0;
    border: 0;
    border-radius: var(--radius-control-compact);
    background: transparent;
    color: var(--color-muted);
    cursor: default;
    transition:
      background-color var(--motion-fast) var(--ease-out),
      color var(--motion-fast) var(--ease-out);
  }

  .bar :global(.tool:hover),
  .bar :global(.tool[data-state="open"]) {
    background: var(--color-fill-hover);
    color: var(--color-text);
  }

  .bar :global(.tool:focus-visible) {
    outline: 2px solid var(--color-ring);
    outline-offset: 1px;
  }

  /* The model reads as words, the effort a step quieter; it is a control
     only under the pointer. */
  .model {
    display: flex;
    min-inline-size: 0;
    margin-inline-end: 2px;
  }

  .bar .model :global(.model-trigger) {
    max-inline-size: 100%;
    block-size: 28px;
    margin: 0;
    padding-inline: 8px;
    gap: 0;
    border-radius: var(--radius-control-compact);
    background: transparent;
    box-shadow: none;
    color: var(--color-label-secondary);
    font-size: 12.5px;
    line-height: 28px;
  }

  .bar .model :global(.model-trigger:hover),
  .bar .model :global(.model-trigger[data-state="open"]) {
    background: var(--color-fill-hover);
    color: var(--color-text);
  }

  .bar .model :global(.model-trigger:active) {
    scale: 1;
  }

  .bar .model :global(.trigger-mark) {
    display: none;
  }

  .bar .model :global(.trigger-name) {
    font-weight: 500;
  }

  .bar .model :global(.trigger-level) {
    color: var(--color-faint);
  }

  .bar .model :global(.trigger-dot) {
    margin-inline: 5px;
  }

  /* Mic, Send and Stop share one place and cross over in it. */
  .action {
    display: grid;
    flex: none;
    place-items: center;
    margin-inline-end: 2px;
  }

  .round {
    display: grid;
    grid-area: 1 / 1;
    place-items: center;
    inline-size: 28px;
    block-size: 28px;
    padding: 0;
    border: 0;
    border-radius: 50%;
    cursor: default;
    opacity: 0;
    scale: 0.7;
    transition:
      background-color var(--motion-fast) var(--ease-out),
      color var(--motion-fast) var(--ease-out),
      opacity var(--motion-fast) var(--ease-out),
      scale var(--motion-base) var(--ease-spring);
  }

  .mic {
    background: transparent;
    color: var(--color-muted);
  }

  .mic:disabled {
    color: var(--color-faint);
  }

  .mic:hover:not(:disabled) {
    background: var(--color-fill-hover);
    color: var(--color-text);
  }

  .mic[aria-pressed="true"] {
    background: var(--color-lit);
    color: var(--color-on-lit);
  }

  .send {
    background: var(--color-lit);
    color: var(--color-on-lit);
  }

  .send:disabled {
    background: var(--color-fill-strong);
    color: var(--color-on-lit);
  }

  .send:hover:not(:disabled) {
    background: var(--color-lit-hover);
  }

  .send:active:not(:disabled) {
    transform: scale(0.9);
  }

  .action[data-action="mic"] .mic,
  .action[data-action="send"] .send:not(.stop),
  .action[data-action="stop"] .stop {
    opacity: 1;
    scale: 1;
  }

  .round:focus-visible {
    outline: 2px solid var(--color-ring);
    outline-offset: 2px;
  }

  @media (prefers-reduced-motion: reduce) {
    .round {
      scale: 1;
      transition: opacity var(--motion-fast) linear;
    }
  }

  :global(:root[data-reduce-motion="true"]) .round {
    scale: 1;
    transition: opacity var(--motion-fast) linear;
  }

  @media (forced-colors: active) {
    .composer,
    .round,
    .chip {
      border: 1px solid ButtonText;
    }
  }
</style>
