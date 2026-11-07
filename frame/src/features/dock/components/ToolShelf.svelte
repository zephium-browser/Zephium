<script lang="ts">
  import * as m from "$shared/i18n/messages";
  import { onMount } from "svelte";
  import { IncognitoIcon, Settings02Icon, UserIcon } from "@hugeicons/core-free-icons";
  import { commands } from "$shared/ipc/bindings";
  import * as tools from "$session/tools.svelte";
  import type { ToolKind } from "$session/tools.svelte";
  import { accountFoot } from "$domain/account";
  import { preferences } from "$domain/preferences";
  import { tabs } from "$domain/tabs";
  import { drawn, profiles } from "$domain/profiles";
  import Disclosure from "$shared/ui/Disclosure";
  import Icon from "$shared/ui/Icon";
  import ProfileAvatar from "$shared/ui/ProfileAvatar";
  import { ASK_TOOL, RECORD_TOOLS, SHELF_TOOLS, toolPresentation } from "../lib/dock-tools";
  import NoticePip from "./NoticePip.svelte";
  import { keymap } from "$domain/keymap";
  import { acceleratorKeys } from "$shared/lib/accelerator";
  import { IS_MAC } from "$shared/platform";

  let { compact = false }: { compact?: boolean } = $props();
  onMount(() => {
    void profiles.retain();
    return () => profiles.release();
  });
  const name = $derived(tabs.profile()?.name ?? profiles.current()?.name ?? "");
  const others = $derived(profiles.inPrivate() ? [] : profiles.others());
  const privately = $derived(profiles.inPrivate() || tabs.profile()?.kind === "incognito");
  /** The private window is open behind the regular tabs, and this brings it back. */
  const aside = $derived(profiles.privateAside());
  const privateLabel = $derived(aside ? m.private_return() : m.browse_incognito_window());
  /** The avatar the profile chose; a private window shows none. */
  const foot = accountFoot({ private: () => privately });
  // A tool whose feature is off is not offered here either.
  const shelf = $derived<ToolKind[]>([
    ...(preferences.aiEnabled() && !privately ? [ASK_TOOL] : []),
    ...SHELF_TOOLS.filter((kind) => preferences.enabled(kind)),
  ]);
  const groups = $derived([shelf, RECORD_TOOLS].filter((group) => group.length > 0));
  // Rows settle in nearest the case first, so each counts from the bottom:
  // Settings, the tools, then who you are.
  const toolRows = $derived(groups.reduce((sum, group) => sum + group.length, 0));
  const headStep = $derived(toolRows + 1);
  function pick(kind: ToolKind) {
    if (tools.activeTool() === kind) tools.close();
    else tools.open(kind);
  }
  // Each row says its key, so the menu teaches the way past itself.
  function keysFor(command: string) {
    const accelerator = keymap.all().find((entry) => entry.id === command)?.accelerator;
    return accelerator ? acceleratorKeys(accelerator, IS_MAC).join(IS_MAC ? "" : "+") : null;
  }
  const named = (label: string, keys: string | null) => (keys ? `${label}  ${keys}` : label);
  const run = (id: string) => void commands.runCommand(id).catch(() => {});
  // The full native menu stays one gesture away, where a Mac user reaches for
  // everything else a control can do.
  function nativeMenu(event: MouseEvent) {
    event.preventDefault();
    const rect = (event.currentTarget as HTMLElement).getBoundingClientRect();
    void commands.toolsMenuPopup(rect.left, rect.top);
  }
</script>

{#snippet row(
  icon: typeof UserIcon,
  label: string,
  keys: string | null,
  step: number,
  onclick: () => void,
  current = false,
)}
  <button
    type="button"
    role="menuitem"
    class="ui-menu-item shelf-item"
    style:--step={step}
    title={compact ? named(label, keys) : undefined}
    aria-label={compact ? label : undefined}
    aria-current={current || undefined}
    {onclick}
  >
    <span class="ui-menu-icon"><Icon {icon} size={16} /></span>
    {#if !compact}<span>{label}</span>{#if keys}<span class="shelf-keys">{keys}</span>{/if}{/if}
  </button>
{/snippet}

{#snippet me(size: number)}{#if foot.face}<ProfileAvatar view={foot.face} {size} />{:else}<span
      class="avatar"
      style:--size={`${size}px`}
      aria-hidden="true"
      ><Icon icon={UserIcon} size={Math.round(size * 0.58)} strokeWidth={1.8} /></span
    >{/if}{/snippet}

<!--
  The foot of the column is you: the profile this window browses as, opening
  onto what belongs to it, stacked on demand. A click opens the stack rising
  out of the avatar, nearest first, the way a stack opens from the Dock;
  nothing opens on hover. Its head is a switcher: you, which opens your
  profiles in Settings, and the others beside you, one click away, with a
  private window last. At rail width the stack is glyphs, each named by its
  tooltip.
-->
<div
  class="shelf"
  data-compact={compact}
  data-morph="profile"
  role="presentation"
  oncontextmenu={nativeMenu}
>
  <Disclosure
    label={name ? `${m.dock_tools()} — ${name}` : m.dock_tools()}
    menu
    triggerClass={["case", tools.activeTool() !== null && "case-lit"].filter(Boolean).join(" ")}
    panelClass="shelf-stack sidebar-softens"
  >
    {#snippet trigger()}{#if foot.face}<ProfileAvatar
          view={foot.face}
          size={compact ? 36 : 40}
        />{:else}<Icon
          icon={UserIcon}
          size={compact ? 16 : 17}
          strokeWidth={1.7}
        />{/if}{#if compact}<NoticePip />{/if}{/snippet}
    <div class="shelf-head" role="presentation" style:--step={headStep}>
      <button
        type="button"
        role="menuitem"
        class="ui-menu-item shelf-me"
        title={compact ? `${name} — ${m.browse_profile_settings()}` : m.browse_profile_settings()}
        aria-label={`${name} — ${m.browse_profile_settings()}`}
        onclick={() => run("settings.profiles")}
      >
        {@render me(compact ? 24 : 22)}
        {#if !compact}<span class="shelf-me-name">{name}</span>{/if}
      </button>
      {#if others.length > 0 || !privately}<div class="shelf-chips" role="presentation">
          {#each others as profile (profile.id)}
            <button
              type="button"
              role="menuitem"
              class="shelf-chip"
              title={profile.name}
              aria-label={m.profiles_menu_label({ name: profile.name })}
              onclick={() => void profiles.switchTo(profile.id)}
              ><ProfileAvatar view={drawn(profile)} size={20} /></button
            >
          {/each}
          {#if !privately}<button
              type="button"
              role="menuitem"
              class="shelf-chip shelf-private"
              data-after-others={others.length > 0 || undefined}
              data-aside={aside || undefined}
              title={named(privateLabel, keysFor("window.newPrivate"))}
              aria-label={privateLabel}
              onclick={() => run("window.newPrivate")}
              ><Icon icon={IncognitoIcon} size={16} strokeWidth={1.7} /></button
            >{/if}
        </div>{/if}
    </div>
    {#each groups as group, at (at)}
      {@const before = groups.slice(0, at).reduce((sum, earlier) => sum + earlier.length, 0)}
      <div class="shelf-rule" role="separator"></div>
      {#each group as kind, index (kind)}
        {@const view = toolPresentation(kind)}
        {@render row(
          view.icon,
          view.label(),
          keysFor(`tool.${kind}`),
          toolRows - before - index,
          () => pick(kind),
          tools.activeTool() === kind,
        )}
      {/each}
    {/each}
    <div class="shelf-rule" role="separator"></div>
    {@render row(Settings02Icon, m.command_browser_settings(), keysFor("browser.settings"), 0, () =>
      run("browser.settings"),
    )}
  </Disclosure>
</div>

<style>
  .shelf {
    position: relative;
    z-index: 20;
    flex: none;
  }

  /* You, at the head of the row: lit the way the current tab is, with light
     caught only along its top edge, and round where every kept site is
     square, so it reads as yours rather than as one more site. */
  .shelf :global(.case) {
    position: relative;
    display: grid;
    place-items: center;
    width: 40px;
    height: 40px;
    margin-inline: 2px;
    border: 0;
    border-radius: var(--radius-capsule);
    background: var(--row-active);
    box-shadow: var(--shine-top);
    color: var(--color-label-secondary);
    cursor: default;
    transition:
      background-color var(--motion-fast) var(--ease-out),
      color var(--motion-fast) var(--ease-out),
      scale var(--motion-slow) var(--ease-spring);
  }

  .shelf :global(.case:hover),
  .shelf :global(.case[data-state="open"]),
  .shelf :global(.case.case-lit) {
    background: var(--row-pressed);
    color: var(--color-text);
  }

  .shelf :global(.case:active) {
    scale: 0.94;
    transition-duration: var(--motion-instant);
  }

  /* A chosen picture fills the lit circle; its light still catches the top. */
  .shelf :global(.case .profile-avatar) {
    pointer-events: none;
  }

  .shelf :global(.case:has(.profile-avatar)::after) {
    position: absolute;
    border-radius: inherit;
    box-shadow: var(--shine-top);
    content: "";
    inset: 0;
    pointer-events: none;
  }

  /* The account glyph where no picture has been chosen, small. */
  .avatar {
    display: grid;
    flex: none;
    width: var(--size);
    height: var(--size);
    border-radius: var(--radius-capsule);
    background: var(--row-active);
    box-shadow: var(--shine-top);
    color: var(--color-label-secondary);
    place-items: center;
  }

  /* The stack rises out of the case rather than dropping from it, and never
     past the window: a short one scrolls its rows instead. */
  .shelf :global(.shelf-stack) {
    top: auto;
    bottom: calc(100% + 8px);
    min-width: min(232px, calc(var(--sidebar-width, 320px) - 8px));
    max-height: calc(100vh - 120px);
    overflow-y: auto;
    overscroll-behavior: contain;
    transform-origin: bottom left;
  }

  .shelf :global(.shelf-stack:not([data-open="true"])) {
    translate: 0 6px;
  }

  /* Who you are, and who else you could be. */
  .shelf-head {
    display: flex;
    align-items: center;
    gap: 4px;
  }

  .shelf-me {
    flex: 1;
    min-width: 0;
    min-height: 34px;
    border: 0;
    background: transparent;
    font: inherit;
    text-align: start;
  }

  .shelf-me-name {
    overflow: hidden;
    color: var(--color-text);
    font-size: var(--text-body);
    font-weight: 600;
    letter-spacing: -0.005em;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .shelf-chips {
    display: flex;
    flex: none;
    align-items: center;
    gap: 2px;
    padding-inline-end: 2px;
  }

  .shelf-chip {
    display: grid;
    width: 28px;
    height: 28px;
    padding: 0;
    border: 0;
    border-radius: 50%;
    background: transparent;
    color: var(--color-muted);
    cursor: default;
    outline: none;
    place-items: center;
    transition:
      background-color var(--motion-instant) var(--ease-smooth),
      color var(--motion-instant) var(--ease-smooth),
      scale var(--motion-base) var(--ease-spring);
  }

  .shelf-chip:hover,
  .shelf-chip:focus-visible {
    background: var(--row-active);
    color: var(--color-text);
  }

  .shelf-chip:focus-visible {
    box-shadow: inset 0 0 0 1.5px var(--color-text);
  }

  .shelf-chip:active {
    scale: 0.92;
    background: var(--row-pressed);
  }

  /* Open and set aside: lit, so the way back reads as a way back. */
  .shelf-private[data-aside] {
    background: var(--row-active);
    color: var(--color-accent);
  }

  /* A private window is not another of your profiles: a hairline keeps it
     apart from them. */
  .shelf-private[data-after-others] {
    position: relative;
    margin-inline-start: 7px;
  }

  .shelf-private[data-after-others]::before {
    position: absolute;
    width: 1px;
    height: 14px;
    background: var(--color-border);
    content: "";
    inset-inline-start: -5px;
  }

  .shelf :global(.shelf-item) {
    min-height: 32px;
    padding-block: 4px;
    width: 100%;
    border: 0;
    background: transparent;
    font: inherit;
    font-size: var(--text-body);
    text-align: start;
  }

  /* A caption, not a keycap: the key belongs to the row and steps forward
     with it when the row is pointed at. */
  .shelf-keys {
    margin-inline-start: auto;
    padding-inline-start: 24px;
    color: var(--color-faint);
    font-size: var(--text-label);
    font-variant-numeric: tabular-nums;
    letter-spacing: 0.02em;
    transition: color var(--motion-instant) var(--ease-smooth);
  }

  .shelf :global(.shelf-item:hover .shelf-keys),
  .shelf :global(.shelf-item:focus-visible .shelf-keys) {
    color: var(--color-muted);
  }

  .shelf :global(:is(.shelf-item, .shelf-me):hover),
  .shelf :global(:is(.shelf-item, .shelf-me):focus-visible) {
    outline: none;
    background: var(--row-active);
  }

  .shelf :global(:is(.shelf-item, .shelf-me):active) {
    background: var(--row-pressed);
  }

  .shelf :global(.shelf-item[aria-current="true"] .ui-menu-icon) {
    color: var(--color-accent);
  }

  /* Each row settles in behind the one below it, so the stack reads as
     unfolding from the case instead of arriving as a block. */
  .shelf :global(.shelf-stack[data-open="true"] :is(.shelf-item, .shelf-head)) {
    animation: shelf-item-in var(--motion-slow) var(--ease-out) both;
    animation-delay: calc(var(--step) * 14ms);
  }

  /* stylelint-disable-next-line keyframes-name-pattern -- Svelte's global prefix. */
  @keyframes -global-shelf-item-in {
    from {
      opacity: 0;
      translate: 0 6px;
    }
  }

  .shelf-rule {
    height: 1px;
    margin: 4px var(--menu-item-inset);
    background: var(--color-border);
  }

  /* At rail width: a square plate like every other rail item, and a stack
     of glyphs just wide enough for them, you and the others first. */
  .shelf[data-compact="true"] :global(.case) {
    width: 36px;
    height: 36px;
    margin-inline: 0;
  }

  /* Centred on the 40px case: four points either side of it. The rail's marks
     and header lie behind it, so it is near-solid rather than glass. */
  .shelf[data-compact="true"] :global(.shelf-stack) {
    background: color-mix(in srgb, var(--color-float) 96%, transparent);
    inset-inline-start: -4px;
    min-width: 0;
    width: 48px;
  }

  .shelf[data-compact="true"] .shelf-head,
  .shelf[data-compact="true"] .shelf-chips {
    flex-direction: column;
    gap: 2px;
    padding: 0;
  }

  .shelf[data-compact="true"] .shelf-me {
    flex: none;
    justify-content: center;
    width: 100%;
    padding-inline: 0;
  }

  .shelf[data-compact="true"] .shelf-chip {
    width: 36px;
    height: 32px;
    border-radius: calc(var(--radius-menu) - var(--menu-pad));
  }

  .shelf[data-compact="true"] .shelf-private[data-after-others] {
    margin: 7px 0 0;
  }

  .shelf[data-compact="true"] .shelf-private[data-after-others]::before {
    width: 14px;
    height: 1px;
    inset-block-start: -4px;
    inset-inline-start: 11px;
  }

  .shelf[data-compact="true"] :global(.shelf-item) {
    justify-content: center;
    padding-inline: 0;
  }

  @media (forced-colors: active) {
    .shelf :global(.case.case-lit) {
      border: 1px solid Highlight;
    }
  }
</style>
