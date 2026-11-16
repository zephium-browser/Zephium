<script lang="ts">
  import { Add01Icon, UserCircleIcon } from "@hugeicons/core-free-icons";
  import type { accountFoot } from "$domain/account";
  import * as m from "$shared/i18n/messages";
  import AccountSummary from "$shared/ui/AccountSummary";
  import Disclosure from "$shared/ui/Disclosure";
  import Icon from "$shared/ui/Icon";

  let {
    account,
    onaccount,
    named = false,
    signInWidth = $bindable(0),
    nameWidth = $bindable(0),
  }: {
    account: ReturnType<typeof accountFoot>;
    onaccount: (intent: "account" | "credits") => void;
    /** Whether there is room for the account's name beside its picture. */
    named?: boolean;
    /** How wide the words of the signed-out button are, for the pane around them. */
    signInWidth?: number;
    /** How wide the name beside the picture is, 0 while it is not shown. */
    nameWidth?: number;
  } = $props();

  // The ring is a warning, not decoration: it is drawn only once the
  // allowance is nearly spent.
  const ringed = $derived(account.tone !== "calm" && account.used != null);
  const name = $derived(named ? account.name : null);
  $effect(() => {
    if (!name) nameWidth = 0;
  });
  const fill = $derived(Math.min(100, Math.max(0, account.used ?? 0)));
  let broken = $state<string | null>(null);
  const photo = $derived(account.avatar && account.avatar !== broken ? account.avatar : null);
  const tip = $derived(
    [m.account_menu(), account.email, account.used != null && m.ai_cloud_used({ percent: fill })]
      .filter(Boolean)
      .join(" · "),
  );

  // A dialog in all but name: Tab goes round the actions instead of leaving
  // them behind an open panel.
  function keep(event: KeyboardEvent) {
    if (event.key !== "Tab") return;
    const items = [
      ...(event.currentTarget as HTMLElement).querySelectorAll<HTMLElement>('[role="menuitem"]'),
    ];
    if (items.length === 0) return;
    event.preventDefault();
    const at = items.indexOf(document.activeElement as HTMLElement);
    const next = at + (event.shiftKey ? -1 : 1);
    items[(next + items.length) % items.length]?.focus();
  }
</script>

{#if account.signedIn}
  <Disclosure
    label={m.account_menu()}
    triggerTitle={tip}
    menu
    align="end"
    triggerClass={name ? "ntp-account ntp-account-named" : "ntp-account"}
    panelClass="ntp-account-panel"
  >
    {#snippet trigger()}<span class="disc" aria-hidden="true"
        >{#if photo}<img
            src={photo}
            alt=""
            draggable="false"
            onerror={() => (broken = photo)}
          />{:else}{account.initials}{/if}</span
      >{#if ringed}<svg
          class="usage-ring"
          data-tone={account.tone}
          viewBox="0 0 32 32"
          aria-hidden="true"
          style:--fill={fill}
          ><circle class="track" cx="16" cy="16" r="15" pathLength="100" /><circle
            class="spent"
            cx="16"
            cy="16"
            r="15"
            pathLength="100"
            transform="rotate(-90 16 16)"
          /></svg
        >{/if}{#if name}<span class="who" bind:offsetWidth={nameWidth}>{name}</span>{/if}{/snippet}
    <div class="body" role="presentation" onkeydown={keep}>
      <div class="summary">
        <AccountSummary
          email={account.email}
          initials={account.initials}
          avatar={account.avatar}
          plan={account.plan}
          used={account.used}
          metered={account.metered}
          tone={account.tone}
        />
      </div>
      <div class="rule" role="separator"></div>
      {#if account.canTopUp}<button
          type="button"
          role="menuitem"
          class="ui-menu-item row"
          onclick={() => onaccount("credits")}
          ><span class="ui-menu-icon"><Icon icon={Add01Icon} size={16} /></span
          >{m.account_credits_open()}</button
        >{/if}
      <button
        type="button"
        role="menuitem"
        class="ui-menu-item row"
        onclick={() => onaccount("account")}
        ><span class="ui-menu-icon"><Icon icon={UserCircleIcon} size={16} /></span
        >{m.account_manage_open()}</button
      >
    </div>
  </Disclosure>
{:else}
  <button
    type="button"
    class="sign-in"
    bind:offsetWidth={signInWidth}
    onclick={() => onaccount("account")}>{m.settings_identity_sign_in()}</button
  >
{/if}

<style>
  /* The account as the page's other control is drawn: a round pane's worth
     of plate, the initials on it. */
  :global(.ntp-account) {
    position: relative;
    display: grid;
    inline-size: 32px;
    block-size: 32px;
    padding: 0;
    border: 0;
    border-radius: var(--radius-capsule);
    background: transparent;
    color: var(--color-text);
    cursor: default;
    outline: none;
    place-items: center;
    transition: scale var(--motion-slow) var(--ease-spring);
  }

  /* With room, the name stands beside the picture inside the same capsule. */
  :global(.ntp-account.ntp-account-named) {
    display: flex;
    gap: 7px;
    inline-size: auto;
    padding-inline: 3px 12px;
  }

  .who {
    max-inline-size: 140px;
    overflow: hidden;
    font-size: 13px;
    font-weight: 500;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  :global(.ntp-account:active) {
    scale: 0.94;
    transition-duration: var(--motion-instant);
  }

  :global(button.ntp-account:focus-visible) {
    outline: none;
    box-shadow: 0 0 0 2px var(--color-ring);
  }

  .disc {
    display: grid;
    inline-size: 26px;
    block-size: 26px;
    border-radius: var(--radius-capsule);
    background: var(--color-fill-active);
    box-shadow: var(--shine-top);
    font-size: var(--text-caption);
    font-weight: 600;
    letter-spacing: 0.03em;
    place-items: center;
    transition: background-color var(--motion-fast) var(--ease-out);
  }

  .disc:has(img) {
    overflow: hidden;
    box-shadow: inset 0 0 0 0.5px var(--avatar-rim);
  }

  :global(.ntp-account:hover) .disc,
  :global(.ntp-account[data-state="open"]) .disc {
    background: var(--color-fill-strong);
  }

  .disc img {
    inline-size: 100%;
    block-size: 100%;
    object-fit: cover;
    animation: interface-fade var(--motion-base) var(--ease-out);
    transition: filter var(--motion-fast) var(--ease-out);
  }

  :global(.ntp-account:hover) .disc img,
  :global(.ntp-account[data-state="open"]) .disc img {
    filter: brightness(1.08);
  }

  .usage-ring {
    position: absolute;
    inset: 0;
    overflow: visible;
    fill: none;
    stroke-width: 1.5;
  }

  .track {
    stroke: var(--color-track);
  }

  .spent {
    stroke: var(--color-warning);
    stroke-dasharray: 100 100;
    stroke-dashoffset: calc(100 - var(--fill));
    stroke-linecap: round;
  }

  .usage-ring[data-tone="full"] .spent {
    stroke: var(--color-danger);
  }

  /* Down out of the corner, close to it and quick: the panel belongs to the
     button it opened from. Solid rather than glass: the page's name is cut
     into the ground beneath it, and words over words through a blur read as
     neither. */
  :global(.ntp-account-panel.ui-disclosure-panel) {
    top: calc(100% + 8px);
    background: var(--color-float);
    backdrop-filter: none;
    inline-size: 272px;
    max-inline-size: calc(100vw - 24px);
    min-inline-size: 0;
    translate: 0 -2px;
  }

  :global(.ntp-account-panel.ui-disclosure-panel[data-open="true"]) {
    translate: 0 0;
    transition:
      opacity var(--motion-fast) var(--ease-out),
      scale 160ms var(--ease-out),
      translate 160ms var(--ease-out),
      visibility 0s;
  }

  .summary {
    padding: 8px var(--menu-item-inset) 12px;
  }

  .rule {
    block-size: 1px;
    margin: 4px var(--menu-item-inset);
    background: var(--color-border);
  }

  .row {
    inline-size: 100%;
    min-block-size: 32px;
    padding-block: 4px;
    border: 0;
    background: transparent;
    font: inherit;
    font-size: var(--text-body);
    text-align: start;
  }

  .row:hover,
  .row:focus-visible {
    outline: none;
    background: var(--row-active);
  }

  .row:active {
    background: var(--row-pressed);
  }

  /* Words in a capsule pane, quieter than the page's own name. */
  .sign-in {
    block-size: 32px;
    padding: 0 14px;
    border: 0;
    border-radius: var(--radius-capsule);
    background: transparent;
    color: var(--color-label-secondary);
    cursor: default;
    font: inherit;
    font-size: var(--text-body);
    font-weight: 500;
    white-space: nowrap;
    outline: none;
    transition:
      background-color var(--motion-instant) var(--ease-smooth),
      color var(--motion-instant) var(--ease-smooth),
      scale var(--motion-slow) var(--ease-spring);
  }

  .sign-in:hover {
    background: var(--color-fill-hover);
    color: var(--color-text);
  }

  .sign-in:active {
    scale: 0.96;
    transition-duration: var(--motion-instant);
  }

  .sign-in:focus-visible {
    outline: none;
    box-shadow: 0 0 0 2px var(--color-ring);
  }

  @media (prefers-reduced-motion: reduce) {
    :global(.ntp-account),
    .sign-in,
    .disc img {
      animation: none;
      transition: none;
    }

    :global(.ntp-account-panel.ui-disclosure-panel) {
      scale: 1;
      translate: 0 0;
    }
  }

  @media (forced-colors: active) {
    .track {
      stroke: GrayText;
    }

    .spent {
      stroke: CanvasText;
    }
  }
</style>
