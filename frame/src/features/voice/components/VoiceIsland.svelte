<script lang="ts">
  import { onDestroy } from "svelte";
  import { fly } from "svelte/transition";
  import AlertCircleIcon from "@hugeicons/core-free-icons/AlertCircleIcon";
  import ArrowUp02Icon from "@hugeicons/core-free-icons/ArrowUp02Icon";
  import Cancel01Icon from "@hugeicons/core-free-icons/Cancel01Icon";
  import HandIcon from "@hugeicons/core-free-icons/HandIcon";
  import MicOff01Icon from "@hugeicons/core-free-icons/MicOff01Icon";
  import StopIcon from "@hugeicons/core-free-icons/StopIcon";
  import Tick02Icon from "@hugeicons/core-free-icons/Tick02Icon";
  import * as m from "$shared/i18n/messages";
  import { voiceSession } from "$domain/voice-session";
  import { keymap } from "$domain/keymap";
  import * as toolHost from "$session/tools.svelte";
  import { acceleratorKeys } from "$shared/lib/accelerator";
  import { reducedMotion } from "$shared/lib/motion";
  import { watchStill } from "$shared/lib/still";
  import { IS_MAC } from "$shared/platform";
  import Button from "$shared/ui/Button";
  import Icon from "$shared/ui/Icon";
  import { Orb, Shimmer } from "$shared/ui/presence";
  import { faultView, freshSplit, island, labelParts, outcomeView } from "../lib/island";
  import * as rail from "../lib/rail-hold";
  import Meter from "./Meter.svelte";

  let {
    compact = false,
    onaccount,
  }: {
    compact?: boolean;
    /** The account page, or its credits, for the fault only the account can fix. */
    onaccount?: (intent: "account" | "credits") => void;
  } = $props();

  const view = $derived(voiceSession.view());
  const model = $derived(island(view));
  const live = $derived(!!view?.mic);

  // Off screen, in a hidden window or with motion reduced the meters hold a
  // resting shape; the colour and the words still say the microphone is on.
  let host = $state<HTMLElement>();
  let still = $state(false);
  $effect(() => {
    const element = host;
    if (!element) return;
    return watchStill(element, (next) => (still = next));
  });
  const REST = 0.42;
  const inputLevel = $derived(still ? REST : voiceSession.inputLevel());
  const outputLevel = $derived(still ? REST : voiceSession.outputLevel());

  const talkKeys = $derived.by(() => {
    const accelerator = keymap.all().find((entry) => entry.id === "voice.talk")?.accelerator;
    return accelerator ? acceleratorKeys(accelerator, IS_MAC) : [];
  });
  const escKey = IS_MAC ? "esc" : "Esc";

  const words = $derived(
    model?.kind === "listening" || model?.kind === "finalizing" ? model.words : null,
  );
  const transcript = $derived(words === null ? null : freshSplit(words));

  // A decision is sent once; the next confirm (or none) asks again.
  let answered = $state(false);
  $effect(() => {
    void (model?.kind === "confirming" ? model.confirm : null);
    answered = false;
  });
  async function decide(decision: "approve" | "decline") {
    answered = true;
    if (!(await voiceSession.decide(decision))) answered = false;
  }

  // The expanded card is the one place a rail's held column gives back.
  $effect(() => {
    if (!compact && model?.kind !== "confirming") rail.release();
  });
  onDestroy(() => {
    if (!compact) rail.release();
  });

  const openConversation = () => toolHost.open("ai");

  function discLabel(): string {
    if (!model) return m.talk_island();
    switch (model.kind) {
      case "listening":
        return model.arming ? m.talk_arming() : m.talk_listening();
      case "finalizing":
        return m.talk_finalizing();
      case "working":
        return model.line;
      case "confirming":
        return m.talk_confirm_label();
      case "speaking":
        return m.talk_speaking();
      case "settled":
        return outcomeView(model.outcome).title;
      case "fault":
        return faultView(model.fault, IS_MAC).title;
      case "warm":
        return m.talk_mic_live();
    }
  }

  function discClick() {
    if (model?.kind === "confirming") rail.hold();
    else openConversation();
  }

  let intent: ReturnType<typeof setTimeout> | undefined;
  function discEnter() {
    if (model?.kind !== "confirming") return;
    clearTimeout(intent);
    intent = setTimeout(rail.hold, 220);
  }
  const discLeave = () => clearTimeout(intent);
  onDestroy(() => clearTimeout(intent));

  function keydown(event: KeyboardEvent) {
    if (event.key !== "Escape" || model?.kind !== "confirming" || !rail.held()) return;
    event.preventDefault();
    rail.release();
  }

  const leave = (node: Element) =>
    fly(node, { y: 6, duration: reducedMotion() ? 0 : 160, opacity: 0 });
</script>

{#snippet keys(caps: readonly string[])}<kbd>{caps.join(IS_MAC ? "" : "+")}</kbd>{/snippet}

{#snippet glyph(size: number)}
  {#if !model}{:else if model.kind === "listening" || model.kind === "finalizing"}<Meter
      kind="input"
      level={model.kind === "listening" && !model.arming ? inputLevel : 0}
      {size}
    />{:else if model.kind === "working"}<Orb
      size={size - 2}
    />{:else if model.kind === "speaking"}<Meter
      kind="output"
      level={outputLevel}
      count={4}
      {size}
    />{:else if model.kind === "confirming"}<Icon
      icon={HandIcon}
      size={size - 2}
      strokeWidth={1.8}
    />{:else if model.kind === "settled"}{@const tone = outcomeView(model.outcome).tone}<Icon
      icon={tone === "done" ? Tick02Icon : tone === "warn" ? AlertCircleIcon : Cancel01Icon}
      size={tone === "quiet" ? size - 6 : size - 2}
      strokeWidth={tone === "done" ? 2.1 : 1.8}
    />{:else if model.kind === "fault"}<Icon
      icon={model.fault === "microphone_denied" || model.fault === "no_microphone"
        ? MicOff01Icon
        : AlertCircleIcon}
      size={size - 2}
      strokeWidth={1.8}
    />{:else}<Meter kind="input" level={0} {size} />{/if}
{/snippet}

<svelte:window onkeydown={keydown} />

{#if model}
  {#if compact}
    <!-- The rail: a disc in the player's place. Its ring is the microphone's
         level while it listens; otherwise it carries a state glyph, and a
         step waiting on the person lights it until the card is opened. -->
    <button
      type="button"
      class="voice-disc"
      bind:this={host}
      data-kind={model.kind}
      data-tone={model.kind === "settled"
        ? outcomeView(model.outcome).tone
        : model.kind === "fault"
          ? "warn"
          : undefined}
      data-live={live || undefined}
      aria-label={`${m.talk_island()}: ${discLabel()}`}
      title={discLabel()}
      onclick={discClick}
      onfocus={() => model?.kind === "confirming" && rail.hold()}
      onpointerenter={discEnter}
      onpointerleave={discLeave}
      out:leave
    >
      <span
        class="voice-ring"
        style:--level={model.kind === "listening" ? inputLevel : 0}
        aria-hidden="true"
      ></span>
      <span class="voice-disc-glyph">{@render glyph(16)}</span>
    </button>
  {:else}
    <section
      class="voice-island"
      bind:this={host}
      aria-label={m.talk_island()}
      data-kind={model.kind}
      data-live={live || undefined}
      out:leave
    >
      {#if model.kind === "confirming"}
        <!-- A security surface: our words around the step's own label, which
             is the page's and is shown as quoted data, never as ours. -->
        <div class="voice-confirm" role="group" aria-label={m.talk_confirm_label()}>
          <div class="voice-row">
            <span class="voice-plate" data-tone="ask">{@render glyph(18)}</span>
            <div class="voice-text">
              <span class="voice-eyebrow">{m.talk_confirm_label()}</span>
              <h3 class="voice-title">{m.talk_confirm_title()}</h3>
            </div>
            {#if live}<span class="voice-live" role="img" aria-label={m.talk_mic_live()}
              ></span>{/if}
          </div>
          <p class="voice-step" data-untrusted>{model.confirm.label}</p>
          <p class="voice-consequence">
            {#if model.confirm.category === "communication"}{m.assist_consequence_communication({
                site: m.assist_this_page(),
              })}{:else if model.confirm.category === "purchase"}{m.assist_consequence_purchase({
                site: m.assist_this_page(),
              })}{:else if model.confirm.category === "destructive"}{m.assist_consequence_destructive(
                { site: m.assist_this_page() },
              )}{:else if model.confirm.category === "save"}{m.assist_consequence_save({
                site: m.assist_this_page(),
              })}{:else if model.confirm.category === "edit"}{m.assist_consequence_edit({
                site: m.assist_this_page(),
              })}{:else if model.confirm.category === "type"}{m.assist_consequence_type({
                site: m.assist_this_page(),
              })}{:else}{m.assist_consequence_other({ site: m.assist_this_page() })}{/if}
          </p>
          <div class="voice-decide">
            <Button
              variant="primary"
              size="compact"
              disabled={answered}
              onclick={() => void decide("approve")}>{m.talk_approve()}</Button
            >
            <Button
              variant="ghost"
              size="compact"
              disabled={answered}
              onclick={() => void decide("decline")}>{m.talk_decline()}</Button
            >
          </div>
          <p class="voice-hint">
            {#if IS_MAC}
              {talkKeys.length
                ? m.talk_confirm_hint({ keys: talkKeys.join("") })
                : m.talk_confirm_hint_plain()}
            {:else}
              {talkKeys.length
                ? m.talk_confirm_hint_spoken({ keys: talkKeys.join("+") })
                : m.talk_confirm_hint_spoken_plain()}
            {/if}
          </p>
        </div>
      {:else}
        <div class="voice-row">
          <span class="voice-announce" role="status">{discLabel()}</span>
          <button type="button" class="voice-body" title={m.talk_open()} onclick={openConversation}>
            <span
              class="voice-plate"
              data-tone={model.kind === "listening" || model.kind === "finalizing"
                ? "live"
                : model.kind === "settled"
                  ? outcomeView(model.outcome).tone
                  : model.kind === "fault"
                    ? "warn"
                    : undefined}
              >{@render glyph(
                18,
              )}{#if live && model.kind !== "listening" && model.kind !== "finalizing"}<span
                  class="voice-live"
                  role="img"
                  aria-label={m.talk_mic_live()}
                ></span>{/if}</span
            >
            <span
              class="voice-text"
              data-wrap={model.kind === "settled" || model.kind === "fault" || model.kind === "warm"
                ? ""
                : undefined}
            >
              {#if model.kind === "listening"}
                <span class="voice-title"
                  >{model.arming ? m.talk_arming() : m.talk_listening()}</span
                >
                {#if model.press === "held"}<span class="voice-sub">{m.talk_hint_held()}</span
                  >{:else if talkKeys.length}<span class="voice-sub voice-keys"
                    >{@render keys(talkKeys)}{m.talk_to_send()}</span
                  >{:else}<span class="voice-sub">{m.talk_hint_latched()}</span>{/if}
              {:else if model.kind === "finalizing"}
                <span class="voice-title"><Shimmer text={m.talk_finalizing()} /></span>
              {:else if model.kind === "working"}
                <span class="voice-title voice-line"><Shimmer text={model.line} /></span>
                {#if model.step}<span class="voice-sub voice-step-line"
                    >{#each labelParts(model.step) as part, at (at)}{#if part.named}<b
                          >“{part.text}”</b
                        >{:else}{part.text}{/if}{/each}</span
                  >{:else if model.heard}<span class="voice-sub voice-heard">“{model.heard}”</span
                  >{/if}
              {:else if model.kind === "speaking"}
                <span class="voice-title"
                  >{model.outcome ? outcomeView(model.outcome).title : m.talk_speaking()}</span
                >
                {#if talkKeys.length}<span class="voice-sub voice-keys"
                    >{@render keys(talkKeys)}{m.talk_to_interrupt()}</span
                  >{:else if model.heard}<span class="voice-sub voice-heard">“{model.heard}”</span
                  >{/if}
              {:else if model.kind === "settled"}
                {@const outcome = outcomeView(model.outcome)}
                <span class="voice-title">{outcome.title}</span>
                {#if outcome.detail}<span class="voice-sub">{outcome.detail}</span>{/if}
              {:else if model.kind === "fault"}
                {@const fault = faultView(model.fault, IS_MAC)}
                <span class="voice-title">{fault.title}</span>
                {#if fault.fix}<span class="voice-sub">{fault.fix}</span>{/if}
              {:else}
                <span class="voice-title">{m.talk_mic_live()}</span>
                <span class="voice-sub">{m.talk_warm()}</span>
              {/if}
            </span>
          </button>
          <span class="voice-actions">
            {#if model.kind === "listening"}
              {#if model.press === "latched"}<button
                  type="button"
                  class="voice-control voice-esc"
                  aria-label={m.talk_cancel()}
                  title={m.talk_cancel()}
                  onclick={() => void voiceSession.intent("cancel")}><kbd>{escKey}</kbd></button
                ><button
                  type="button"
                  class="voice-control voice-send"
                  aria-label={m.talk_send()}
                  title={m.talk_send()}
                  onclick={() => void voiceSession.intent("stop")}
                  ><Icon icon={ArrowUp02Icon} size={14} strokeWidth={2.2} /></button
                >{:else}<button
                  type="button"
                  class="voice-control"
                  aria-label={m.talk_cancel()}
                  title={m.talk_cancel()}
                  onclick={() => void voiceSession.intent("cancel")}
                  ><Icon icon={Cancel01Icon} size={12} strokeWidth={2.2} /></button
                >{/if}
            {:else if model.kind === "working" || model.kind === "speaking"}
              {#if model.kind === "working" && model.speaking}<span class="voice-voiceover"
                  ><Meter kind="output" level={outputLevel} count={3} size={12} /></span
                >{/if}
              <button
                type="button"
                class="voice-control"
                aria-label={model.kind === "speaking"
                  ? m.talk_stop_speaking()
                  : m.talk_stop_working()}
                title={model.kind === "speaking" ? m.talk_stop_speaking() : m.talk_stop_working()}
                onclick={() => void voiceSession.intent("cancel")}
                ><Icon icon={StopIcon} size={10} /></button
              >
            {:else if model.kind === "fault"}
              <button
                type="button"
                class="voice-control"
                aria-label={m.talk_dismiss()}
                title={m.talk_dismiss()}
                onclick={() => void voiceSession.intent("cancel")}
                ><Icon icon={Cancel01Icon} size={12} strokeWidth={2.2} /></button
              >
            {/if}
          </span>
        </div>
        {#if transcript}
          <p
            class="voice-words"
            data-selectable
            data-empty={!words || undefined}
            data-settled={model.kind === "finalizing" || undefined}
          >
            <span class="flow"
              >{#if words}<span class="said">{transcript.said}</span><span class="newest"
                  >{transcript.newest}</span
                >{:else}{m.talk_listening_empty()}{/if}</span
            >
          </p>
        {/if}
        {#if model.kind === "fault"}
          {@const remedy = faultView(model.fault, IS_MAC).remedy}
          {#if remedy && onaccount}<div class="voice-remedy">
              <Button
                size="compact"
                onclick={() => onaccount(remedy === "credits" ? "credits" : "account")}
                >{remedy === "credits"
                  ? m.talk_fault_buy_credits()
                  : m.talk_fault_see_plans()}</Button
              >
            </div>{/if}
        {/if}
      {/if}
    </section>
  {/if}
{/if}

<style>
  /* The card is the media card's sibling: the same plate, rim and inset, so
     the two stack at the foot of the column as one family. Only the plate on
     its leading edge changes with the state, and only the meter moves. */
  .voice-island {
    --voice-live: var(--color-soft-peach);
    --voice-live-wash: color-mix(in oklab, var(--voice-live) 16%, transparent);

    position: relative;
    display: flex;
    flex: none;
    flex-direction: column;
    gap: 6px;
    margin: 6px var(--sidebar-inset);
    padding: 8px 6px 8px 8px;
    border-radius: var(--radius-card);
    background: var(--row-active);
    box-shadow: var(--row-rim);
    animation: island-in var(--motion-base) var(--ease-emphasized) both;
  }

  /* While the microphone listens the card's edge takes its colour: the one
     place in the chrome that says it is live. */
  .voice-island[data-kind="listening"] {
    box-shadow:
      var(--row-rim),
      inset 0 0 0 1px color-mix(in oklab, var(--voice-live) 34%, transparent);
  }

  .voice-row {
    display: flex;
    align-items: center;
    gap: 4px;
    min-inline-size: 0;
  }

  .voice-body {
    display: flex;
    flex: 1;
    align-items: center;
    gap: 8px;
    min-inline-size: 0;
    margin: -2px;
    padding: 2px;
    border: 0;
    border-radius: var(--radius-inset);
    background: transparent;
    color: inherit;
    font: inherit;
    text-align: start;
    cursor: default;
  }

  .voice-body:focus-visible {
    outline: 2px solid var(--color-accent);
    outline-offset: 1px;
  }

  .voice-plate {
    position: relative;
    display: grid;
    flex: none;
    place-items: center;
    inline-size: 30px;
    block-size: 30px;
    border-radius: var(--radius-inset);
    background: var(--color-fill);
    color: var(--color-label-secondary);
    transition:
      background-color var(--motion-base) var(--ease-out),
      color var(--motion-base) var(--ease-out);
  }

  .voice-body:hover .voice-plate {
    background: var(--row-pressed);
  }

  .voice-plate[data-tone="live"] {
    background: var(--voice-live-wash);
    color: var(--voice-live);
  }

  .voice-plate[data-tone="done"] {
    color: var(--color-text);
  }

  .voice-plate[data-tone="warn"] {
    color: var(--color-warning);
  }

  .voice-plate[data-tone="ask"] {
    background: var(--color-lit);
    color: var(--color-on-lit);
  }

  .voice-plate :global(.orb) {
    --orb-tone: var(--color-text);
  }

  /* The microphone is open while nothing listens: a dot of the live colour
     on the plate's shoulder, as the system marks a live microphone. */
  .voice-live {
    inline-size: 7px;
    block-size: 7px;
    border-radius: 50%;
    background: var(--voice-live);
    box-shadow: 0 0 0 2px var(--color-chrome);
  }

  .voice-plate .voice-live {
    position: absolute;
    inset-block-start: -2px;
    inset-inline-end: -2px;
  }

  .voice-text {
    display: grid;
    flex: 1;
    min-inline-size: 0;
  }

  .voice-title,
  .voice-sub {
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .voice-title {
    margin: 0;
    color: var(--color-text);
    font-size: var(--text-label);
    font-weight: 550;
    line-height: 17px;
  }

  .voice-line {
    --shimmer-rest: var(--color-muted);
    --shimmer-lit: var(--color-text);

    font-weight: 500;
  }

  .voice-sub {
    color: var(--color-muted);
    font-size: var(--text-caption);
    line-height: 16px;
  }

  .voice-heard {
    color: var(--color-faint);
  }

  /* The step the reply is on: the page's words for the control, quoted. */
  .voice-step-line {
    display: -webkit-box;
    color: var(--color-label-secondary);
    white-space: normal;
    -webkit-box-orient: vertical;
    -webkit-line-clamp: 2;
    line-clamp: 2;
  }

  .voice-step-line b {
    color: var(--color-text);
    font-weight: 500;
  }

  .voice-text[data-wrap] :is(.voice-title, .voice-sub) {
    white-space: normal;
    text-wrap: pretty;
  }

  .voice-announce {
    position: absolute;
    inline-size: 1px;
    block-size: 1px;
    overflow: hidden;
    clip-path: inset(50%);
    white-space: nowrap;
  }

  .voice-remedy {
    display: flex;
    padding-inline-start: 38px;
  }

  .voice-keys {
    display: flex;
    align-items: center;
    gap: 4px;
  }

  kbd {
    display: inline-grid;
    place-items: center;
    box-sizing: border-box;
    min-inline-size: 16px;
    block-size: 15px;
    padding: 0 3px;
    border-radius: 4px;
    background: var(--color-fill-strong);
    color: var(--color-label-secondary);
    font-family: inherit;
    font-size: 10.5px;
    font-weight: 600;
    line-height: 1;
  }

  .voice-esc kbd {
    min-inline-size: 22px;
    block-size: 18px;
    background: var(--color-fill);
  }

  .voice-esc:hover kbd {
    background: transparent;
    color: var(--color-text);
  }

  /* What is being said: the last two lines, the newest words strongest, the
     older ones fading up out of the card. */
  .voice-words {
    display: flex;
    align-items: flex-end;
    max-block-size: 40px;
    margin: 0 2px 0 0;
    overflow: hidden;
    color: var(--color-text);
    font-size: 13px;
    letter-spacing: -0.003em;
    line-height: 20px;
    overflow-wrap: anywhere;
    mask-image: linear-gradient(to bottom, transparent 0, black 14px);
    /* stylelint-disable-next-line property-no-vendor-prefix */
    -webkit-user-select: text;
    user-select: text;
  }

  .flow {
    display: block;
    min-inline-size: 0;
  }

  .voice-words .said {
    color: var(--color-muted);
    transition: color var(--motion-base) var(--ease-out);
  }

  .voice-words[data-settled] .said {
    color: var(--color-text);
  }

  .voice-words[data-empty] {
    color: var(--color-faint);
    mask-image: none;
  }

  .voice-actions {
    display: flex;
    flex: none;
    align-items: center;
    gap: 2px;
  }

  .voice-voiceover {
    display: grid;
    place-items: center;
    inline-size: 20px;
    color: var(--color-muted);
  }

  .voice-voiceover :global(.meter) {
    --meter-ink: var(--color-muted);
  }

  .voice-control {
    display: grid;
    flex: none;
    place-items: center;
    inline-size: 26px;
    block-size: 26px;
    padding: 0;
    border: 0;
    border-radius: var(--radius-capsule);
    background: transparent;
    color: var(--color-label-secondary);
    cursor: default;
    transition:
      background-color var(--motion-fast) var(--ease-out),
      color var(--motion-fast) var(--ease-out),
      scale var(--motion-slow) var(--ease-spring);
  }

  .voice-control:hover {
    background: var(--row-pressed);
    color: var(--color-text);
  }

  .voice-control:active {
    scale: 0.88;
    transition-duration: var(--motion-instant);
  }

  .voice-control:focus-visible {
    outline: 2px solid var(--color-accent);
    outline-offset: 1px;
  }

  .voice-send {
    background: var(--color-lit);
    color: var(--color-on-lit);
  }

  .voice-send:hover {
    background: var(--color-lit-hover);
    color: var(--color-on-lit);
  }

  /* The confirmation: the card grows into it, our question above the step's
     own words, which sit apart as quoted data. */
  .voice-confirm {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 2px 2px 2px 0;
  }

  .voice-confirm .voice-row {
    gap: 8px;
  }

  .voice-eyebrow {
    color: var(--color-muted);
    font-size: var(--text-caption);
    line-height: 15px;
  }

  .voice-confirm .voice-title {
    font-size: var(--text-page-title);
    font-weight: 600;
    letter-spacing: -0.01em;
    line-height: 19px;
    white-space: normal;
  }

  .voice-confirm .voice-live {
    align-self: flex-start;
    margin: 4px 4px 0 0;
  }

  .voice-step {
    margin: 0;
    padding: 7px 10px;
    border-radius: var(--radius-inset);
    background: var(--color-fill);
    box-shadow: inset 0 0 0 0.5px var(--color-border);
    color: var(--color-text);
    font-size: 13px;
    font-weight: 500;
    line-height: 18px;
    overflow-wrap: anywhere;
    white-space: pre-wrap;
  }

  .voice-consequence,
  .voice-hint {
    margin: 0;
    color: var(--color-muted);
    font-size: 12.5px;
    line-height: 17px;
    text-wrap: pretty;
  }

  .voice-hint {
    color: var(--color-faint);
    font-size: var(--text-caption);
    line-height: 15px;
  }

  .voice-decide {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }

  /* The rail's disc, in the player's place and on its plate. */
  .voice-disc {
    --voice-live: var(--color-soft-peach);

    position: relative;
    display: grid;
    flex: none;
    place-items: center;
    align-self: center;
    inline-size: 36px;
    block-size: 36px;
    margin-block-start: 8px;
    padding: 0;
    border: 0;
    border-radius: var(--radius-capsule);
    background: var(--row-active);
    box-shadow: var(--shine-top);
    color: var(--color-label-secondary);
    cursor: default;
    animation: island-in var(--motion-base) var(--ease-emphasized) both;
    transition:
      background-color var(--motion-fast) var(--ease-out),
      color var(--motion-fast) var(--ease-out),
      scale var(--motion-slow) var(--ease-spring);
  }

  .voice-disc:focus-visible {
    outline: 2px solid var(--color-accent);
    outline-offset: 1px;
  }

  .voice-disc:hover {
    background: var(--row-pressed);
    color: var(--color-text);
  }

  .voice-disc:active {
    scale: 0.92;
    transition-duration: var(--motion-instant);
  }

  .voice-disc[data-kind="working"],
  .voice-disc[data-kind="speaking"] {
    color: var(--color-text);
  }

  .voice-disc[data-tone="done"] {
    color: var(--color-text);
  }

  .voice-disc[data-tone="warn"] {
    color: var(--color-warning);
  }

  /* Waiting on the person: lit like the current tab's plate inverted, which
     nothing else in the rail is, until the card opens. */
  .voice-disc[data-kind="confirming"] {
    background: var(--color-lit);
    box-shadow: var(--shadow-badge);
    color: var(--color-on-lit);
  }

  .voice-ring {
    position: absolute;
    inset: 0;
    border-radius: inherit;
    box-shadow: inset 0 0 0 1.5px var(--color-fill-strong);
    pointer-events: none;
  }

  .voice-disc[data-live] .voice-ring {
    box-shadow: inset 0 0 0 1.5px var(--voice-live);
  }

  /* Listening, a second ring breathes out with the voice: drawn by each
     level event, still between them. */
  .voice-disc[data-kind="listening"] .voice-ring::after {
    position: absolute;
    inset: 0;
    border-radius: inherit;
    box-shadow: 0 0 0 2px color-mix(in oklab, var(--voice-live) 45%, transparent);
    content: "";
    opacity: calc(0.25 + var(--level) * 0.75);
    transform: scale(calc(1 + var(--level) * 0.16));
    transition:
      transform 90ms linear,
      opacity 90ms linear;
  }

  .voice-disc-glyph {
    display: grid;
    place-items: center;
  }

  .voice-disc :global(.orb) {
    --orb-tone: var(--color-text);
  }

  @keyframes island-in {
    from {
      opacity: 0;
      translate: 0 6px;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .voice-island,
    .voice-disc {
      animation-duration: 1ms;
    }

    .voice-disc[data-kind="listening"] .voice-ring::after {
      transition: none;
    }
  }

  :global(:root[data-reduce-motion="true"]) .voice-island,
  :global(:root[data-reduce-motion="true"]) .voice-disc {
    animation-duration: 1ms;
  }

  @media (forced-colors: active) {
    .voice-island,
    .voice-disc {
      border: 1px solid CanvasText;
    }

    .voice-live {
      background: Highlight;
    }
  }
</style>
