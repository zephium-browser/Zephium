<script lang="ts">
  import * as m from "$shared/i18n/messages";
  import type { Ask } from "./asks";
  import type { AskActions, ConfirmDecision } from "./actions";
  import ConfirmCard from "./ConfirmCard.svelte";
  import ConnectionCard from "./ConnectionCard.svelte";
  import ContextCard from "./ContextCard.svelte";
  import EntryCard from "./EntryCard.svelte";
  import FolderCard from "./FolderCard.svelte";
  import AddressCard from "./AddressCard.svelte";
  import QuestionCard from "./QuestionCard.svelte";
  import SignInCard from "./SignInCard.svelte";

  /**
   * Any question a run puts to the person, on the thing it concerns or in the
   * island. A decision is sent once; the card holds until Rust's projection
   * shows it taken, and says so in one line if it was refused.
   */
  let {
    ask,
    actions,
    placement = "canvas",
    seed = 0,
  }: {
    ask: Ask;
    actions: AskActions;
    placement?: "canvas" | "island";
    /** The asking agent's orb, for its own questions. */
    seed?: number;
  } = $props();

  let sending = $state<string | null>(null);
  let refused = $state<string | null>(null);
  const busy = $derived(sending === ask.step && ask.state === "open");

  async function send(work: () => Promise<boolean>) {
    if (sending) return;
    sending = ask.step;
    refused = null;
    const sent = await work().catch(() => false);
    if (!sent) {
      refused = ask.step;
      sending = null;
    }
  }
  $effect(() => {
    if (ask.state !== "open") sending = null;
  });
  const decide = (decision: ConfirmDecision) => send(() => actions.confirm(ask.step, decision));
  const answer = (text: string) => send(() => actions.answer(ask.step, text));
  /** The system's folder panel; closing it leaves the question open, as it was. */
  async function choose(start: string) {
    if (sending || !actions.chooseFolder) return;
    const path = await actions.chooseFolder(start).catch(() => null);
    if (path) await answer(path);
  }
</script>

<div class="ask-card" data-ask={ask.kind} data-step={ask.step}>
  {#if ask.kind === "confirm"}
    <ConfirmCard
      {ask}
      {placement}
      {busy}
      ondecide={decide}
      onopenpage={actions.openPage ? () => actions.openPage?.(ask.step) : undefined}
    />
  {:else if ask.kind === "entry"}
    <EntryCard {ask} {placement} {busy} onanswer={answer} />
  {:else if ask.kind === "context"}
    <ContextCard {ask} {placement} {busy} onanswer={answer} />
  {:else if ask.kind === "connection"}
    <ConnectionCard {ask} {placement} {busy} onanswer={answer} />
  {:else if ask.kind === "folder"}
    <FolderCard
      {ask}
      {placement}
      {busy}
      onanswer={answer}
      onchoose={actions.chooseFolder ? () => void choose(ask.path) : undefined}
    />
  {:else if ask.kind === "address"}
    <AddressCard {ask} {placement} {busy} onanswer={answer} />
  {:else if ask.kind === "sign_in"}
    <SignInCard
      {ask}
      {placement}
      {busy}
      onopen={actions.openPage ? () => actions.openPage?.(ask.step) : undefined}
      onsignedin={actions.signedIn ? () => void send(() => actions.signedIn!(ask.page)) : undefined}
    />
  {:else}
    <QuestionCard {ask} {placement} {busy} {seed} onanswer={answer} />
  {/if}
  {#if refused === ask.step && ask.state === "open"}<p class="refused" role="alert">
      {m.work_ask_refused()}
    </p>{/if}
</div>

<style>
  .ask-card {
    display: flex;
    flex-direction: column;
    gap: 6px;
    min-inline-size: 0;
  }

  .refused {
    margin: 0;
    padding-inline: 4px;
    color: var(--color-warning);
    font-size: var(--text-label);
    line-height: 16px;
  }
</style>
