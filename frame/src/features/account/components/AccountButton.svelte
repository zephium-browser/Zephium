<script lang="ts">
  import { onMount } from "svelte";
  import { CubeIcon, Settings02Icon, UserCircleIcon, UserIcon } from "@hugeicons/core-free-icons";
  import { accountFoot, accountSession } from "$domain/account";
  import { keymap } from "$domain/keymap";
  import { preferences } from "$domain/preferences";
  import { drawn, profiles } from "$domain/profiles";
  import { tabs } from "$domain/tabs";
  import { commands } from "$shared/ipc/bindings";
  import * as m from "$shared/i18n/messages";
  import { acceleratorKeys } from "$shared/lib/accelerator";
  import { IS_MAC } from "$shared/platform";
  import AccountSummary from "$shared/ui/AccountSummary";
  import Disclosure from "$shared/ui/Disclosure";
  import Badge from "$shared/ui/Badge";
  import Icon from "$shared/ui/Icon";
  import ProfileAvatar from "$shared/ui/ProfileAvatar";
  import "$shared/ui/Shine/shine.css";

  /** The profile's name until the window's own profile is known. */
  let {
    name = "",
    onvoice,
  }: {
    name?: string;
    /** Opens Voice settings; without it the menu offers no Voice row. */
    onvoice?: () => void;
  } = $props();

  onMount(() => {
    void accountSession.retain();
    void profiles.retain();
    return () => {
      accountSession.release();
      profiles.release();
    };
  });

  const privately = $derived(profiles.inPrivate() || tabs.profile()?.kind === "incognito");
  const foot = accountFoot({ private: () => privately });
  const who = $derived(tabs.profile()?.name || profiles.current()?.name || name);
  const shown = $derived(who || m.browse_profile_settings());
  const others = $derived(profiles.others());
  const tone = $derived(foot.tone);
  const voice = $derived(!!onvoice && preferences.aiEnabled() && !privately);
  // Rows settle in from the trigger: Settings first, you last.
  const lift = $derived(voice ? 1 : 0);

  function keysFor(command: string) {
    const accelerator = keymap.all().find((entry) => entry.id === command)?.accelerator;
    return accelerator ? acceleratorKeys(accelerator, IS_MAC).join(IS_MAC ? "" : "+") : null;
  }
  const run = (id: string) => void commands.runCommand(id).catch(() => {});
</script>

{#snippet face(size: number)}{#if foot.face}<ProfileAvatar view={foot.face} {size} />{:else}<span
      class="plain"
      style:--size="{size}px"
      aria-hidden="true"
      ><Icon icon={UserIcon} size={Math.round(size * 0.58)} strokeWidth={1.8} /></span
    >{/if}{/snippet}

{#snippet row(
  icon: typeof UserIcon,
  label: string,
  caption: string | null,
  step: number,
  go: () => void,
)}
  <button type="button" role="menuitem" class="ui-menu-item row" style:--step={step} onclick={go}>
    <span class="ui-menu-icon"><Icon {icon} size={16} /></span>
    <span class="label">{label}</span>
    {#if caption}<span class="caption">{caption}</span>{/if}
  </button>
{/snippet}

<!--
  Work's foot is the same figure as Browse's: you, as a lit circle, opening onto
  who you are and what belongs to the work: your account, its models, settings.
  It is the rail's own light menu rather than the page-wide one, since it
  loads with Work into a chrome that pays for every dependency at launch.
-->
<Disclosure
  label={m.account_menu()}
  menu
  triggerClass="work-case"
  panelClass="work-menu ui-shine-sheet sidebar-softens"
>
  {#snippet trigger()}{#if foot.face}<ProfileAvatar view={foot.face} size={36} />{:else}<Icon
        icon={UserIcon}
        size={16}
        strokeWidth={1.7}
      />{/if}{#if tone !== "calm"}<span class="pip" data-tone={tone} aria-hidden="true"
      ></span>{/if}{/snippet}
  <div class="head" role="presentation" style:--step={3 + lift}>
    <button
      type="button"
      role="menuitem"
      class="ui-menu-item me"
      aria-label={`${shown} — ${m.browse_profile_settings()}`}
      title={m.browse_profile_settings()}
      onclick={() => run("settings.profiles")}
    >
      {@render face(22)}
      <span class="me-name">{shown}</span>
    </button>
    {#if others.length > 0}<div class="chips" role="presentation">
        {#each others as profile (profile.id)}
          <button
            type="button"
            role="menuitem"
            class="chip"
            title={profile.name}
            aria-label={m.profiles_menu_label({ name: profile.name })}
            onclick={() => void profiles.switchTo(profile.id)}
            ><ProfileAvatar view={drawn(profile)} size={20} /></button
          >
        {/each}
      </div>{/if}
  </div>
  <div class="rule" role="separator"></div>
  {#if foot.available && !privately}
    {#if foot.signedIn}<button
        type="button"
        role="menuitem"
        class="ui-menu-item account"
        style:--step={2 + lift}
        aria-label={`${m.account_menu()} — ${foot.email ?? ""}`}
        onclick={() => run("settings.account")}
        ><AccountSummary
          email={foot.email}
          initials={foot.initials}
          avatar={foot.avatar}
          plan={foot.plan}
          used={foot.used}
          metered={foot.metered}
          tone={foot.tone}
        /></button
      >{:else}{@render row(UserCircleIcon, m.settings_sign_in(), null, 2 + lift, () =>
        run("settings.account"),
      )}{/if}
    <div class="rule" role="separator"></div>
  {/if}
  {@render row(CubeIcon, m.settings_models(), null, 1 + lift, () => run("settings.ai"))}
  <div class="rule" role="separator"></div>
  {#if voice}<button
      type="button"
      role="menuitem"
      class="ui-menu-item row"
      style:--step={1}
      onclick={() => onvoice?.()}
    >
      <span class="ui-menu-icon"
        ><svg class="voice-glyph" viewBox="0 0 16 16" width="16" height="16" aria-hidden="true"
          ><rect x="2.25" y="5.5" width="2" height="5" rx="1" /><rect
            x="5.5"
            y="2.5"
            width="2"
            height="11"
            rx="1"
          /><rect x="8.75" y="4" width="2" height="8" rx="1" /><rect
            x="12"
            y="6.25"
            width="2"
            height="3.5"
            rx="1"
          /></svg
        ></span
      >
      <span class="label">{m.talk_menu()}</span>
      <span class="new"><Badge tone="accent">{m.talk_new()}</Badge></span>
    </button>{/if}
  {@render row(Settings02Icon, m.command_browser_settings(), keysFor("browser.settings"), 0, () =>
    run("browser.settings"),
  )}
</Disclosure>

<style>
  /* Up out of the trigger, clear of it and of the window's foot, the way the
     tool stack rises from Browse's: fully on screen at any rail or column width. */
  :global(.work-menu.ui-disclosure-panel) {
    top: auto;
    bottom: calc(100% + 12px);
    inset-inline-start: 0;
    min-width: min(232px, calc(100vw - 16px));
    max-width: min(320px, calc(100vw - 16px));
    max-height: calc(100vh - 140px);
    overflow-y: auto;
    overscroll-behavior: contain;
    transform-origin: bottom left;
    translate: 0 6px;
  }

  :global(.work-menu.ui-disclosure-panel[data-open="true"]) {
    translate: 0 0;
  }

  /* The profile as the tab list's own foot draws it: lit like the current tab,
     with light caught only along its top edge. */
  :global(.work-case) {
    position: relative;
    display: grid;
    inline-size: 36px;
    block-size: 36px;
    padding: 0;
    border: 0;
    border-radius: var(--radius-capsule);
    background: var(--row-active);
    box-shadow: var(--shine-top);
    color: var(--color-label-secondary);
    place-items: center;
    cursor: default;
    pointer-events: auto;
    transition:
      background-color var(--motion-fast) var(--ease-out),
      color var(--motion-fast) var(--ease-out),
      scale var(--motion-slow) var(--ease-spring);
  }

  :global(.work-case:hover),
  :global(.work-case[data-state="open"]) {
    background: var(--row-pressed);
    color: var(--color-text);
  }

  :global(.work-case:active) {
    scale: 0.94;
    transition-duration: var(--motion-instant);
  }

  :global(.work-case:focus-visible) {
    outline: 2px solid var(--color-ring);
    outline-offset: 1px;
  }

  :global(.work-case .profile-avatar) {
    pointer-events: none;
  }

  :global(.work-case:has(.profile-avatar)::after) {
    position: absolute;
    border-radius: inherit;
    box-shadow: var(--shine-top);
    content: "";
    inset: 0;
    pointer-events: none;
  }

  /* The agent stops when the included usage runs out, so the figure says so from the rail. */
  .pip {
    position: absolute;
    inset-block-end: 0;
    inset-inline-end: 0;
    inline-size: 9px;
    block-size: 9px;
    border-radius: 50%;
    background: var(--color-warning, var(--color-soft-lemon));
    box-shadow: 0 0 0 2px var(--color-chrome);
  }

  .pip[data-tone="full"] {
    background: var(--color-danger);
  }

  .plain {
    display: grid;
    flex: none;
    inline-size: var(--size);
    block-size: var(--size);
    border-radius: var(--radius-capsule);
    background: var(--row-active);
    box-shadow: var(--shine-top);
    color: var(--color-label-secondary);
    place-items: center;
  }

  .head {
    display: flex;
    align-items: center;
    gap: 4px;
  }

  .me {
    flex: 1;
    min-inline-size: 0;
    min-block-size: 34px;
    border: 0;
    background: transparent;
    font: inherit;
    text-align: start;
  }

  .me-name {
    overflow: hidden;
    color: var(--color-text);
    font-size: var(--text-body);
    font-weight: 600;
    letter-spacing: -0.005em;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .chips {
    display: flex;
    flex: none;
    align-items: center;
    gap: 2px;
    padding-inline-end: 2px;
  }

  .chip {
    display: grid;
    inline-size: 28px;
    block-size: 28px;
    padding: 0;
    border: 0;
    border-radius: 50%;
    background: transparent;
    cursor: default;
    outline: none;
    place-items: center;
    transition:
      background-color var(--motion-instant) var(--ease-smooth),
      scale var(--motion-base) var(--ease-spring);
  }

  .chip:hover,
  .chip:focus-visible {
    background: var(--row-active);
  }

  .chip:focus-visible {
    box-shadow: inset 0 0 0 1.5px var(--color-text);
  }

  .chip:active {
    scale: 0.92;
    background: var(--row-pressed);
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

  .label {
    white-space: nowrap;
  }

  .voice-glyph {
    fill: currentcolor;
  }

  .new {
    display: flex;
    margin-inline-start: auto;
    padding-inline-start: 16px;
  }

  .new :global(.ui-badge) {
    padding-block: 0;
    font-size: 10.5px;
  }

  /* A caption, not a keycap: it belongs to the row and says what stands behind it. */
  .caption {
    overflow: hidden;
    margin-inline-start: auto;
    padding-inline-start: 24px;
    color: var(--color-faint);
    font-size: var(--text-label);
    font-variant-numeric: tabular-nums;
    letter-spacing: 0.02em;
    text-overflow: ellipsis;
    white-space: nowrap;
    transition: color var(--motion-instant) var(--ease-smooth);
  }

  .row:hover .caption,
  .row:focus-visible .caption {
    color: var(--color-muted);
  }

  :is(.row, .me, .account):hover,
  :is(.row, .me, .account):focus-visible {
    outline: none;
    background: var(--row-active);
  }

  :is(.row, .me, .account):active {
    background: var(--row-pressed);
  }

  /* The account summed up, as a row that opens the rest of it. */
  .account {
    display: block;
    inline-size: 100%;
    padding: 10px var(--menu-item-inset) 12px;
    border: 0;
    background: transparent;
    font: inherit;
  }

  .rule {
    block-size: 1px;
    margin: 4px var(--menu-item-inset);
    background: var(--color-border);
  }

  /* Each row settles in behind the one below it, so the menu unfolds from its foot. */
  :global(.work-menu[data-open="true"]) :is(.row, .head, .account) {
    animation: account-row-in var(--motion-slow) var(--ease-out) both;
    animation-delay: calc(var(--step) * 14ms);
  }

  /* stylelint-disable-next-line keyframes-name-pattern -- Svelte's global prefix. */
  @keyframes -global-account-row-in {
    from {
      opacity: 0;
      translate: 0 6px;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    :global(.work-case),
    .chip,
    .caption {
      transition: none;
    }

    :global(.work-menu[data-open="true"]) :is(.row, .head, .account) {
      animation: none;
    }
  }

  @media (forced-colors: active) {
    :global(.work-case[data-state="open"]) {
      border: 1px solid Highlight;
    }
  }
</style>
