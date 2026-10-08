<script lang="ts">
  import * as m from "$shared/i18n/messages";
  import {
    ArrowLeft02Icon,
    ArrowRight02Icon,
    EllipsisIcon,
    Refresh01Icon,
    Search01Icon,
    SidebarLeftIcon,
  } from "@hugeicons/core-free-icons";
  import { tabs } from "$domain/tabs";
  import { blocker, siteMenuState } from "$domain/blocker";
  import { commands } from "$shared/ipc/bindings";
  import { IS_MAC, IS_WINDOWS } from "$shared/platform";
  import IconButton from "$shared/ui/IconButton";
  import ModeTabs from "./ModeTabs.svelte";
  import WindowControls from "$shared/ui/WindowControls";

  let {
    compact,
    ontoggle,
    navigation = true,
    pageControls = true,
    launcher = false,
  }: {
    compact: boolean;
    ontoggle: () => void;
    navigation?: boolean;
    /** Back, forward and reload act on a page; Work shows none. */
    pageControls?: boolean;
    /** The rail beside a tool panel has no head of its own to carry search. */
    launcher?: boolean;
  } = $props();

  let active = $derived(tabs.activeTab());

  function openSidebarMenu(event: MouseEvent) {
    const target = event.currentTarget;
    if (!(target instanceof HTMLButtonElement)) return;
    const anchor = target.getBoundingClientRect();
    const { siteProtected, canHide } = siteMenuState(blocker.status());
    void commands.sidebarMenuPopup(anchor.left, anchor.bottom, siteProtected, canHide);
  }
</script>

<!--
  macOS keeps AppKit's traffic lights through the overlay title bar, so the
  leading 52px of the top row belongs to the system. Windows reveals native
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
    {#if IS_MAC}
      <div style:height="var(--titlebar-height)" aria-hidden="true"></div>
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
      <IconButton
        icon={Search01Icon}
        label={m.ui_search_or_enter_an_address()}
        size={15}
        buttonSize={26}
        onclick={() => void commands.runCommand("launcher.toggle")}
      />
    </div>
    <span class="mt-[6px] mb-[7px] h-px w-[22px] rounded-full bg-border" aria-hidden="true"></span>
  </header>
{:else}
  <header
    class="flex shrink-0 items-center gap-px pe-1.5"
    style:height="var(--titlebar-height)"
    style:padding-inline-start={IS_MAC ? "var(--traffic-light-inset)" : "6px"}
    aria-label={m.ui_navigation()}
  >
    <!-- Beside a tool panel the panel's own close button already gives the
         column back, so a second way to the same shape would only crowd the
         lights. -->
    {#if navigation && !launcher}<IconButton
        icon={SidebarLeftIcon}
        label={m.ui_compact_mode()}
        onclick={ontoggle}
      />
    {/if}<!-- The search glyph reads closer to the lights than the toggle's
         does, so it keeps a little more room. -->{#if launcher}<span
        class="ms-1.5 flex"
        ><IconButton
          icon={Search01Icon}
          label={m.ui_search_or_enter_an_address()}
          onclick={() => void commands.runCommand("launcher.toggle")}
        /></span
      >
    {/if}<span class="flex-1" aria-hidden="true"></span>

    {#if navigation && pageControls}<div
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
        />
        <IconButton
          icon={ArrowRight02Icon}
          label={m.ui_forward()}
          shape="rounded"
          buttonSize={28}
          size={16}
          disabled={active?.can_go_forward !== true}
          onclick={tabs.forwardActive}
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
