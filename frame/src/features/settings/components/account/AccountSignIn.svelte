<script lang="ts">
  import { Alert02Icon, InformationCircleIcon } from "@hugeicons/core-free-icons";
  import { accountSession as session } from "$domain/account";
  import { commands } from "$shared/ipc/bindings";
  import * as m from "$shared/i18n/messages";
  import Button from "$shared/ui/Button";
  import Field from "$shared/ui/Field";
  import Icon from "$shared/ui/Icon";
  import {
    PRIVACY_URL,
    TERMS_URL,
    codeDigits,
    explainAccountFault,
    plausibleEmail,
  } from "../../lib/account";

  /** What the page has to say before anyone signs in: a session that ended, an
   *  address that moved, a server that wants a newer Zephium. */
  let { notices = [] }: { notices?: { text: string; tone?: "warning" }[] } = $props();

  const uid = $props.id();
  const account = $derived(session.account());
  const pendingEmail = $derived(account?.pending_email ?? null);
  const fault = $derived(session.fault());
  const busy = $derived(session.pending());

  let email = $state("");
  let code = $state("");
  let resent = $state(false);
  let emailForm = $state<HTMLFormElement | null>(null);
  let codeForm = $state<HTMLFormElement | null>(null);
  const focus = (form: HTMLFormElement | null) => form?.querySelector("input")?.focus();
  let now = $state(Date.now());
  const resendAt = $derived(Date.now() + (account?.resend_in ?? 0) * 1000);
  $effect(() => {
    if (resendAt <= now) return;
    const timer = setInterval(() => (now = Date.now()), 1000);
    return () => clearInterval(timer);
  });
  const resendIn = $derived(Math.max(0, Math.ceil((resendAt - now) / 1000)));

  $effect(() => {
    if (pendingEmail) focus(codeForm);
  });

  /** An invited-only server sends no code to an address it has not invited, and says nothing. */
  const mode = $derived(account?.signup_mode ?? null);
  const faultText = $derived(
    fault && (fault.action === "send" || fault.action === "verify")
      ? explainAccountFault(fault.fault, fault.retryAfter, mode)
      : null,
  );
  const emailValid = $derived(plausibleEmail(email));
  const step = $derived(pendingEmail ? "code" : "email");
  /** The fault is about what was typed, so the field it came from says so too. */
  const faulty = $derived(
    fault?.fault === "invalid_code" ||
      fault?.fault === "invalid" ||
      (fault?.action === "send" && fault.fault === "signup_closed"),
  );

  async function send() {
    if (!emailValid || busy) return;
    resent = false;
    await session.sendCode(email.trim());
  }

  async function resend() {
    if (!pendingEmail || busy || resendIn > 0) return;
    code = "";
    resent = await session.sendCode(pendingEmail);
    focus(codeForm);
  }

  async function verify() {
    const digits = codeDigits(code);
    if (digits.length !== 6 || busy) return;
    resent = false;
    const ok = await session.verify(digits);
    if (!ok) {
      code = "";
      focus(codeForm);
    }
  }

  function typed(value: string) {
    const digits = codeDigits(value);
    code = digits;
    if (digits.length === 6) void verify();
  }

  async function otherEmail() {
    code = "";
    resent = false;
    if (pendingEmail) email = pendingEmail;
    await session.cancelCode();
    focus(emailForm);
  }

  function open(url: string) {
    void commands.browserOpenUrl(url, true).catch(() => {});
  }
</script>

<div class="sign-in" data-step={step} aria-busy={busy === "send" || busy === "verify" || undefined}>
  <div class="panes">
    <div class="pane" role="group" aria-labelledby={`${uid}-email`} inert={step !== "email"}>
      <h2 id={`${uid}-email`}>{m.account_welcome_title()}</h2>
      <p class="body">{m.account_welcome_body()}</p>
      <form
        class="entry"
        data-invalid={step === "email" && faulty && fault?.action === "send"}
        bind:this={emailForm}
        onsubmit={(event) => {
          event.preventDefault();
          void send();
        }}
      >
        <Field
          label={m.account_email()}
          labelHidden
          size="large"
          type="email"
          placeholder={m.account_email_placeholder()}
          autocomplete="email"
          spellcheck="false"
          autocapitalize="off"
          maxlength={254}
          bind:value={email}
          disabled={busy === "send"}
        />
        <Button
          type="submit"
          variant="primary"
          size="large"
          pending={busy === "send"}
          disabled={!emailValid}>{m.account_continue()}</Button
        >
      </form>
    </div>
    <div class="pane" role="group" aria-labelledby={`${uid}-code`} inert={step !== "code"}>
      <h2 id={`${uid}-code`}>{m.account_code_title()}</h2>
      <p class="body">{m.account_code_body({ email: pendingEmail ?? "" })}</p>
      <form
        class="entry"
        data-invalid={step === "code" && faulty && fault?.action === "verify"}
        bind:this={codeForm}
        onsubmit={(event) => {
          event.preventDefault();
          void verify();
        }}
      >
        <Field
          class="code"
          label={m.account_code()}
          labelHidden
          size="large"
          inputmode="numeric"
          autocomplete="one-time-code"
          placeholder="000000"
          maxlength={11}
          spellcheck="false"
          value={code}
          oninput={(event) => typed(event.currentTarget.value)}
          disabled={busy === "verify"}
        />
        <Button
          type="submit"
          variant="primary"
          size="large"
          pending={busy === "verify"}
          disabled={codeDigits(code).length !== 6}>{m.account_continue()}</Button
        >
      </form>
    </div>
  </div>

  <!-- Everything the page has to say stands here, in room kept for it, so a
       message arriving moves nothing. -->
  <div class="status">
    {#each notices.filter((notice) => notice.text !== faultText) as notice (notice.text)}
      <p class="note" data-tone={notice.tone} role="status">
        <Icon icon={InformationCircleIcon} size={14} />{notice.text}
      </p>
    {/each}
    {#if step === "email"}
      {#if mode === "allowlist"}<p class="note" role="note">
          <Icon icon={InformationCircleIcon} size={14} />{m.account_signup_invite_note()}
        </p>
      {:else if mode === "closed"}<p class="note" role="note">
          <Icon icon={InformationCircleIcon} size={14} />{m.account_signup_closed_note()}
        </p>{/if}
    {:else}
      {#if resent && !faultText}<p class="note" role="status">
          <Icon icon={InformationCircleIcon} size={14} />{m.account_code_sent()}
        </p>{/if}
      {#if mode === "allowlist" && !faultText}<p class="note" role="note">
          <Icon icon={InformationCircleIcon} size={14} />{m.account_signup_invite_note()}
        </p>{/if}
    {/if}
    {#if faultText}<p class="note" data-tone="error" role="alert">
        <Icon icon={Alert02Icon} size={14} />{faultText}
      </p>{/if}
  </div>

  <div class="foot">
    <p class="pane terms" inert={step !== "email"}>
      {m.account_terms()}
      <a
        href={TERMS_URL}
        onclick={(event) => {
          event.preventDefault();
          open(TERMS_URL);
        }}>{m.account_terms_link()}</a
      >
      ·
      <a
        href={PRIVACY_URL}
        onclick={(event) => {
          event.preventDefault();
          open(PRIVACY_URL);
        }}>{m.account_privacy_link()}</a
      >
    </p>
    <div class="pane links" inert={step !== "code"}>
      <Button
        variant="ghost"
        size="compact"
        disabled={resendIn > 0 || !!busy}
        pending={busy === "send"}
        onclick={() => void resend()}
        >{resendIn > 0
          ? m.account_code_resend_in({ seconds: resendIn })
          : m.account_code_resend()}</Button
      >
      <span class="dot" aria-hidden="true">·</span>
      <Button variant="ghost" size="compact" disabled={!!busy} onclick={() => void otherEmail()}
        >{m.account_code_other()}</Button
      >
    </div>
  </div>
</div>

<style>
  /* One sheet of glass for both steps. They stand in the same place and
     trade by opacity and a short slide, so nothing in the sheet changes
     size and the page behind it stays still. */
  .sign-in {
    --control-large: 44px;

    inline-size: min(100%, 400px);
    padding: 36px 32px 0;
    border-radius: var(--radius-panel);
    background: var(--sky-card);
    backdrop-filter: blur(28px) saturate(1.3);
    box-shadow:
      inset 0 1px 0 var(--sky-card-rim),
      0 0 0 0.5px var(--sky-card-edge),
      var(--shadow-float);
    color: var(--color-text);
    text-align: center;
  }

  .panes,
  .foot {
    display: grid;
  }

  .pane {
    --slide: 14px;

    grid-area: 1 / 1;
    min-inline-size: 0;
    visibility: visible;
    transition:
      opacity var(--motion-slow) var(--ease-out) 60ms,
      translate var(--motion-page) var(--ease-emphasized),
      visibility 0s;
  }

  /* The step that is not showing waits to one side of the one that is: the
     address leaves backwards, the code arrives from ahead. */
  .pane[inert] {
    visibility: hidden;
    opacity: 0;
    transition:
      opacity var(--motion-base) var(--ease-out),
      translate var(--motion-page) var(--ease-emphasized),
      visibility 0s linear var(--motion-page);
    translate: calc(var(--slide) * -1) 0;
  }

  .sign-in[data-step="email"] .pane[inert] {
    translate: var(--slide) 0;
  }

  .sign-in:dir(rtl) .pane {
    --slide: -14px;
  }

  h2 {
    margin: 0;
    color: var(--color-text);
    font-size: 22px;
    font-weight: 600;
    line-height: 1.25;
    letter-spacing: -0.022em;
  }

  .body {
    margin: 8px 0 24px;
    color: var(--color-muted);
    font-size: var(--text-body);
    line-height: 1.55;
    overflow-wrap: anywhere;
    text-wrap: balance;
  }

  .entry {
    display: grid;
    gap: 10px;
  }

  .entry :global(.ui-input) {
    text-align: center;
  }

  .entry :global(.code .ui-input) {
    font-family: var(--font-mono);
    font-size: 22px;
    font-variant-numeric: tabular-nums;
    letter-spacing: 0.5em;
    text-indent: 0.5em;
  }

  .entry[data-invalid="true"] :global(.ui-input) {
    box-shadow: inset 0 0 0 1px var(--color-danger);
  }

  .entry :global(.ui-button) {
    transition:
      background-color var(--motion-base) var(--ease-smooth),
      color var(--motion-base) var(--ease-smooth),
      scale var(--motion-fast) var(--ease-smooth);
  }

  /* Until there is an address to send to, the button waits as a quiet plate
     and lights when there is; while it works, it stays lit and shows it. */
  .entry :global(.ui-button[data-variant="primary"]:disabled) {
    background: var(--color-fill-active);
    color: var(--color-faint);
    opacity: 1;
  }

  .entry :global(.ui-button[data-variant="primary"][aria-busy="true"]) {
    background: var(--color-lit);
    color: var(--color-on-lit);
  }

  .status {
    display: grid;
    align-content: start;
    gap: 6px;
    box-sizing: border-box;
    min-block-size: 44px;
    padding-block-end: 8px;
    margin-block-start: 8px;
  }

  .note {
    display: flex;
    align-items: baseline;
    justify-content: center;
    gap: 6px;
    margin: 0;
    color: var(--color-muted);
    font-size: var(--text-label);
    line-height: 18px;
    text-align: start;
    text-wrap: pretty;
    animation: note-in var(--motion-slow) var(--ease-out) backwards;
  }

  .note :global(svg) {
    flex: none;
    translate: 0 2px;
  }

  .note[data-tone="error"] {
    color: var(--color-danger);
  }

  .note[data-tone="warning"] {
    color: var(--color-warning);
  }

  @keyframes note-in {
    from {
      opacity: 0;
      translate: 0 4px;
    }
  }

  /* The footing of the sheet: what the first step agrees to, or what the
     second can do instead. */
  .foot {
    padding-block: 14px 16px;
    border-block-start: 1px solid var(--sky-card-edge);
  }

  .links {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 2px;
    font-variant-numeric: tabular-nums;
  }

  .dot {
    color: var(--color-faint);
  }

  .terms {
    align-self: center;
    margin: 0;
    color: var(--color-faint);
    font-size: var(--text-caption);
    line-height: 24px;
  }

  .terms a {
    border-radius: var(--radius-swatch);
    color: var(--color-muted);
    text-decoration: underline;
    text-decoration-color: var(--color-border-strong);
    text-underline-offset: 2px;
  }

  .terms a:hover {
    color: var(--color-text);
  }

  .terms a:focus-visible {
    outline: 2px solid var(--color-ring);
    outline-offset: 2px;
  }

  /* The global switch collapses durations to a frame; the wait before a
     step is hidden would still stand, so it goes as well. */
  @media (prefers-reduced-motion: reduce) {
    .pane[inert] {
      transition-delay: 0s;
    }
  }

  :global(:root[data-reduce-motion="true"]) .pane[inert] {
    transition-delay: 0s;
  }

  @media (forced-colors: active) {
    .sign-in {
      border: 1px solid CanvasText;
      background: Canvas;
      backdrop-filter: none;
      box-shadow: none;
    }

    .foot {
      border-block-start-color: CanvasText;
    }

    .entry :global(.ui-button[data-variant="primary"]:disabled) {
      background: Canvas;
      color: GrayText;
    }

    .entry :global(.ui-button[data-variant="primary"][aria-busy="true"]) {
      background: Highlight;
      color: HighlightText;
    }

    .entry[data-invalid="true"] :global(.ui-input) {
      outline: 2px dashed CanvasText;
    }
  }
</style>
