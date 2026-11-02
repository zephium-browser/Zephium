<script lang="ts">
  import { onMount, type Component, type Snippet } from "svelte";
  import type { ToolHostProps } from "$session/tool-drafts.svelte";
  import { tabs } from "$domain/tabs";
  import { surface } from "$domain/surface";
  import { commands } from "$shared/ipc/bindings";
  import { events } from "$shared/ipc/native-events";
  import * as m from "$shared/i18n/messages";
  import IconButton from "$shared/ui/IconButton";
  import Icon from "$shared/ui/Icon";
  import Menu from "$shared/ui/Menu";
  import { AssistSession } from "../lib/assist-session.svelte";
  import {
    Cancel01Icon,
    Clock01Icon,
    Delete02Icon,
    MoreHorizontalIcon,
    PaintBoardIcon,
    PencilEdit02Icon,
    Settings02Icon,
  } from "../lib/icons";
  import Composer from "./Composer.svelte";
  import HistoryView from "./HistoryView.svelte";
  import SiteStyles from "./SiteStyles.svelte";
  import Thread from "./Thread.svelte";

  /** The tool frame is the host's: the panel fills it with the thread and the composer. */
  type Frame = Component<
    ToolHostProps & {
      caption?: boolean;
      scrolls?: boolean;
      closable?: boolean;
      actions?: Snippet;
      footer?: Snippet;
      children?: Snippet;
    }
  >;
  let { Frame, ...viewProps }: ToolHostProps & { Frame: Frame } = $props();

  let session = $state.raw<AssistSession | null>(null);
  $effect(() => {
    const owner = new AssistSession(viewProps.profile);
    session = owner;
    void owner.start();
    return () => owner.dispose();
  });
  // One conversation per Space; the tab in front is what the page chip offers.
  const space = $derived(tabs.activeSpaceId() ?? null);
  const active = $derived(tabs.activeId() ?? null);
  $effect(() => session?.follow(space, active));
  // A navigation changes what the page is, not whose conversation this is.
  const address = $derived(tabs.activeTab()?.url ?? null);
  $effect(() => {
    void address;
    session?.refresh();
  });
  $effect(() => {
    const urls = new Map(tabs.tabs().map((tab) => [tab.id, tab.url]));
    session?.forget((tab) => urls.get(tab));
  });

  let view = $state<"thread" | "history" | "styles">("thread");
  $effect(() => {
    void space;
    view = "thread";
  });

  let composer = $state<ReturnType<typeof Composer>>();
  onMount(() => {
    let stop: (() => void) | null = null;
    let live = true;
    // Asking for the panel again, while it is open, goes to its field.
    void events.uiCommand
      .listen(({ payload }) => {
        if (payload !== "tool.ai") return;
        view = "thread";
        composer?.focus();
      })
      .then((off) => {
        if (live) stop = off;
        else off();
      });
    return () => {
      live = false;
      stop?.();
    };
  });

  /** A started Work opens in Work mode, on that Work. */
  async function openWork(work: string) {
    const current = session?.space;
    if (current) {
      const { environmentSession } = await import("$domain/work-environment");
      environmentSession(viewProps.profile, current)?.show(work);
    }
    void surface.open("work");
  }

  const empty = $derived(!session?.conversation.messages.length);
  const entries = $derived([
    { kind: "item" as const, id: "history", label: m.assist_history(), icon: Clock01Icon },
    {
      kind: "item" as const,
      id: "styles",
      label: m.assist_menu_styles(),
      icon: PaintBoardIcon,
      disabled: !session?.status?.page,
    },
    { kind: "item" as const, id: "models", label: m.assist_menu_models(), icon: Settings02Icon },
    { kind: "separator" as const },
    {
      kind: "item" as const,
      id: "delete",
      label: m.assist_menu_delete(),
      icon: Delete02Icon,
      danger: true,
      disabled: empty,
    },
    { kind: "separator" as const },
    {
      kind: "item" as const,
      id: "close",
      label: m.assist_menu_close(),
      icon: Cancel01Icon,
      hint: "esc",
    },
  ]);

  function choose(id: string) {
    if (id === "history" || id === "styles") view = id;
    else if (id === "models") void commands.runCommand("settings.ai").catch(() => {});
    else if (id === "close") viewProps.onclose();
    else if (id === "delete") {
      view = "thread";
      void session?.deleteCurrent();
    }
  }
</script>

<!-- Escape steps back from a sheet, then closes the panel; the composer takes it first to stop. -->
<svelte:window
  onkeydown={(event) => {
    if (event.key !== "Escape" || event.defaultPrevented || event.isComposing) return;
    if (session?.picking) {
      event.preventDefault();
      session.cancelPick();
      return;
    }
    if (view !== "thread") view = "thread";
    else viewProps.onclose();
  }}
/>

<div class="assist-host" data-assist role="presentation">
  <Frame {...viewProps} caption={false} scrolls={false} closable={false}>
    {#snippet actions()}
      <IconButton icon={Cancel01Icon} label={m.tool_close()} onclick={viewProps.onclose} />
      <IconButton
        icon={PencilEdit02Icon}
        label={m.assist_new()}
        disabled={empty}
        onclick={() => {
          view = "thread";
          void session?.newConversation();
          composer?.focus();
        }}
      />
      <Menu
        label={m.assist_more()}
        {entries}
        align="end"
        triggerClass="assist-more"
        onselect={choose}
      >
        {#snippet trigger()}<Icon icon={MoreHorizontalIcon} size={16} />{/snippet}
      </Menu>
    {/snippet}
    {#if session}
      {#if view === "history"}<HistoryView {session} onback={() => (view = "thread")} />
      {:else if view === "styles"}<SiteStyles {session} onback={() => (view = "thread")} />
      {:else}<Thread
          {session}
          onwork={(work) => void openWork(work)}
          onexit={() => composer?.focus()}
        />{/if}
    {/if}
    {#snippet footer()}
      {#if session}<Composer
          {session}
          draft={viewProps.state.draft}
          ondraft={(draft) => viewProps.edit({ draft })}
          bind:this={composer}
        />{/if}
    {/snippet}
  </Frame>
</div>

<style>
  .assist-host {
    display: contents;
  }

  .assist-host :global(.assist-more) {
    display: grid;
    place-items: center;
    inline-size: 28px;
    block-size: 28px;
    padding: 0;
    border: 0;
    border-radius: var(--radius-control);
    background: transparent;
    color: var(--color-muted);
    cursor: default;
    transition:
      background-color var(--motion-instant) var(--ease-smooth),
      color var(--motion-instant) var(--ease-smooth);
  }

  .assist-host :global(.assist-more:hover),
  .assist-host :global(.assist-more[data-state="open"]) {
    background: var(--color-fill-hover);
    color: var(--color-text);
  }

  /* While one of the panel's menus is open, the column beside it steps back,
     as it does under the sidebar's own panels. */
  :global(.browser-sidebar:has(.assist-host [aria-expanded="true"]) [data-under-panel]) {
    opacity: 0.18;
  }

  .assist-host :global(.assist-more:focus-visible) {
    outline: 2px solid var(--color-ring);
  }
</style>
