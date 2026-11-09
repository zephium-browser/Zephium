<script lang="ts">
  import * as m from "$shared/i18n/messages";
  import {
    ArrowLeft02Icon,
    ArrowRight02Icon,
    EllipsisIcon,
    Refresh01Icon,
    SidebarLeftIcon,
  } from "@hugeicons/core-free-icons";
  import { tabs } from "$domain/tabs";
  import { blocker, siteMenuState } from "$domain/blocker";
  import { pageSite } from "$domain/site-data";
  import { extensions } from "$domain/extensions";
  import { webext } from "$domain/webext";
  import { uiCommands as ui } from "$domain/ui-commands";
  import { untrack } from "svelte";
  import { commands } from "$shared/ipc/bindings";
  import { IS_MAC, IS_WINDOWS } from "$shared/platform";
  import { fullscreen } from "$shared/lib/fullscreen.svelte";
  import IconButton from "$shared/ui/IconButton";
  import ModeTabs from "./ModeTabs.svelte";
  import SidebarSearch from "./SidebarSearch.svelte";
  import WindowControls from "$shared/ui/WindowControls";

  let {
    compact,
    ontoggle,
    pageControls = true,
    launcher = false,
  }: {
    compact: boolean;
    ontoggle: () => void;
    /** Back, forward and reload act on a page; Work shows none. */
    pageControls?: boolean;
    /** Beside a tool panel, search stands where the shape toggle would. */
    launcher?: boolean;
  } = $props();

  let active = $derived(tabs.activeTab());
  // In fullscreen the lights are gone, and so is the room kept for them.
  let lights = $derived(IS_MAC && !fullscreen());

  // At rail width the extensions' buttons live in this menu; a popup chosen
  // there hangs from the button that opened it.
  let menuAnchor: DOMRect | null = null;
  function openSidebarMenu(event: MouseEvent) {
    const target = event.currentTarget;
    if (!(target instanceof HTMLButtonElement)) return;
    const anchor = target.getBoundingClientRect();
    menuAnchor = anchor;
    const { siteProtected, canHide } = siteMenuState(blocker.status());
    // A private window keeps nothing to clear, and only a web page has a site.
    const canClearData =
      tabs.profile()?.kind !== "incognito" &&
      (active?.content ?? "web") === "web" &&
      pageSite(active?.url) !== null;
    const listed = extensions
      .activeActions(tabs.profile()?.id ?? null, tabs.activeId())
      .map((action) => ({
        id: action.runtime.install_id,
        label: action.badge ? `${action.label} (${action.badge})` : action.label,
        icon: action.icon_rgba_base64,
        enabled: action.enabled,
      }));
    void commands.sidebarMenuPopup(
      anchor.left,
      anchor.bottom,
      siteProtected,
      canHide,
      canClearData,
      listed,
      webext.isAvailable(),
    );
  }

  // Back and Forward hold the pages behind and ahead; a held press or a right
  // click lists them in a native menu.
  function openHistoryMenu(forward: boolean, anchor: DOMRect) {
    const id = tabs.activeId();
    if (id) void commands.historyMenuPopup(id, forward, anchor.left, anchor.bottom);
  }

  let handledCommand = 0;
  $effect(() => {
    const command = ui.uiCommand();
    if (command.seq === 0 || command.seq === handledCommand) return;
    handledCommand = command.seq;
    if (!command.id.startsWith("extension.action=")) return;
    const id = command.id.slice("extension.action=".length);
    untrack(() => {
      const profile = tabs.profile()?.id ?? null;
      const tab = tabs.activeId();
      const action = extensions
        .activeActions(profile, tab)
        .find((candidate) => candidate.runtime.install_id === id);
      if (profile === null || tab === null || !action || !menuAnchor) return;
      void extensions.invoke(profile, tab, action, menuAnchor);
    });
  });
</script>

<!--
  macOS keeps AppKit's traffic lights through the overlay title bar, so the
  leading 52px of the top row belongs to the system, except in fullscreen,
  where AppKit hides them. Windows reveals native
  controls at the top-right edge. On Linux the controls are ours, and at rail width they move into the collapsed menu
  because three buttons cannot fit beside anything else.
-->
{#if compact}
  <!--
    The rail's head: which environment you are in, then the two controls that
    act on the column. At 56px two 26px buttons are exactly what fits on one
    line, and the rule below them says where the head ends.
  -->
  <header class="flex shrink-0 flex-col items-center gap-[3px]" aria-label={m.ui_navigation()}>
    {#if lights}
      <div style:height="var(--titlebar-height)" aria-hidden="true"></div>
    {:else if IS_MAC}
      <div style:height="8px" aria-hidden="true"></div>
    {/if}
    <ModeTabs compact />
    <div class="flex items-center gap-0.5">
      <!-- Its menu acts on the page and the column's shape; Work has neither. -->
      {#if pageControls}<IconButton
          icon={EllipsisIcon}
          label={m.ui_sidebar_options()}
          size={15}
          buttonSize={26}
          haspopup
          onclick={openSidebarMenu}
        />{/if}
      <SidebarSearch small />
    </div>
    <span class="mt-[6px] mb-[7px] h-px w-[22px] rounded-full bg-border" aria-hidden="true"></span>
  </header>
{:else}
  <header
    class="flex shrink-0 items-center gap-px pe-1.5"
    style:height="var(--titlebar-height)"
    style:padding-inline-start={lights
      ? "var(--traffic-light-inset)"
      : launcher && IS_WINDOWS
        ? "0"
        : "6px"}
    aria-label={m.ui_navigation()}
  >
    <!-- Beside a tool panel the panel's own close button already gives the
         column back, so a second way to the same shape would only crowd the
         lights; search stands there instead. -->
    {#if launcher && IS_WINDOWS}<span class="flex w-14 shrink-0 justify-center"
        ><ModeTabs compact /></span
      >{:else if launcher}<span class="ms-1.5 flex"><SidebarSearch /></span>{:else}<IconButton
        icon={SidebarLeftIcon}
        label={m.ui_compact_mode()}
        onclick={ontoggle}
      />{/if}<span class="flex-1" aria-hidden="true"></span>

    {#if pageControls}<div
        class="flex items-center gap-0.5"
        role="group"
        aria-label={m.ui_navigation()}
      >
        <IconButton
          icon={ArrowLeft02Icon}
          label={m.ui_back()}
          shape="rounded"
          buttonSize={28}
          size={16}
          disabled={active?.can_go_back !== true}
          onclick={tabs.backActive}
          onmenu={(anchor) => openHistoryMenu(false, anchor)}
        />
        <IconButton
          icon={ArrowRight02Icon}
          label={m.ui_forward()}
          shape="rounded"
          buttonSize={28}
          size={16}
          disabled={active?.can_go_forward !== true}
          onclick={tabs.forwardActive}
          onmenu={(anchor) => openHistoryMenu(true, anchor)}
        />
        <IconButton
          icon={Refresh01Icon}
          label={m.ui_reload()}
          shape="rounded"
          buttonSize={28}
          size={16}
          disabled={active === undefined}
          onclick={tabs.reloadActive}
        />
      </div>{/if}

    {#if !IS_MAC && !IS_WINDOWS}
      <WindowControls />
    {/if}
  </header>
{/if}
