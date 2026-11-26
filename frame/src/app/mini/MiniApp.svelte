<script lang="ts">
  import RenderBoundary from "$shared/ui/RenderBoundary";
  import "$styles/mini.css";
  import { onMount, flushSync, tick } from "svelte";
  import type { MiniIntent, TabView } from "$shared/ipc/bindings";
  import { commands } from "$shared/ipc/bindings";
  import { events } from "$shared/ipc/native-events";
  import { theme } from "$domain/appearance";
  import {
    acceptMiniTab,
    acceptMiniView,
    miniShortcut,
    MiniToolbar,
    windowOfLabel,
  } from "$features/mini";
  import * as m from "$shared/i18n/messages";
  import { applyLanguage } from "$shared/lib/locale.svelte";
  import { IS_MAC } from "$shared/platform";

  let { label }: { label: string } = $props();
  // Native names the window by this label; it never changes.
  const id = $derived(windowOfLabel(label));

  let tab = $state<TabView | null>(null);
  let spaces = $state(0);
  let pinned = $state(false);

  // The presentation barrier reads this DOM back in the same turn.
  function show(next: TabView | null) {
    flushSync(() => {
      tab = next;
    });
  }
  function act(intent: MiniIntent) {
    void commands.miniAction(intent).catch(() => {});
  }
  function spaceMenu(x: number, y: number) {
    void commands.miniSpaceMenu(x, y).catch(() => {});
  }
  // The page usually holds focus, and native routes the window's keys from
  // there. These answer while the toolbar holds it.
  function onkeydown(event: KeyboardEvent) {
    const intent = miniShortcut(event, IS_MAC);
    if (!intent || (!tab && intent !== "close")) return;
    event.preventDefault();
    act(intent);
  }

  onMount(() => {
    let disposed = false;
    const listeners: (() => void)[] = [];
    async function track(registration: Promise<() => void>) {
      const stop = await registration;
      if (disposed) stop();
      else listeners.push(stop);
    }
    function dispose() {
      if (disposed) return;
      disposed = true;
      for (const stop of listeners) stop();
      listeners.length = 0;
      theme.dispose();
    }
    let liveMotion = false;
    let liveLanguage = false;
    const liveLook: string[] = [];
    const uiListener = events.uiCommand.listen(({ payload }) => {
      if (disposed) return;
      const look = theme.customizationKeys.find((key) => payload.startsWith(`preference.${key}=`));
      if (look) {
        liveLook.push(look);
        theme.customize(look, payload.slice(`preference.${look}=`.length));
      }
      if (payload.startsWith("preference.ui.language=")) {
        liveLanguage = true;
        void applyLanguage(payload.slice("preference.ui.language=".length));
      }
      if (
        payload === "preference.ui.reduce-motion=true" ||
        payload === "preference.ui.reduce-motion=false"
      ) {
        liveMotion = true;
        theme.setReducedMotion(payload.endsWith("=true"));
      }
    });
    const viewListener = events.mini.listen(({ payload }) => {
      if (disposed || id === null || payload.window !== id) return;
      spaces = payload.spaces.length;
      pinned = payload.pinned;
      if (acceptMiniView(id, tab, payload)) show(payload.tab);
    });
    const presentationListener = events.miniPresentation.listen(({ payload }) => {
      if (!disposed && acceptMiniTab(tab, payload.tab)) show(payload.tab);
    });

    void (async () => {
      try {
        await Promise.all([viewListener, presentationListener, uiListener].map(track));
        if (disposed) return;
        await theme.init();
        if (disposed) return;
        const [motion, language, ...look] = await Promise.all([
          commands.settingGet("ui.reduce-motion").catch(() => null),
          commands.settingGet("ui.language").catch(() => null),
          ...theme.customizationKeys.map((key) => commands.settingGet(key).catch(() => null)),
        ]);
        if (disposed) return;
        theme.customizationKeys.forEach((key, index) => {
          const value = look[index];
          if (!liveLook.includes(key) && value != null) theme.customize(key, value);
        });
        if (!liveMotion) theme.setReducedMotion(motion === "true");
        if (!liveLanguage) await applyLanguage(language);
        await tick();
        if (disposed) return;
        void document.documentElement.getBoundingClientRect();
        // Native shows the window only now, so its first frame is drawn.
        await commands.miniReady();
      } catch {
        // The window stays hidden; closing the app removes it.
        dispose();
      }
    })();
    return dispose;
  });
</script>

<svelte:window {onkeydown} />

<div class="mini-root">
  <RenderBoundary title={m.surface_render_failed()} retryLabel={m.surface_retry()}>
    <MiniToolbar
      {tab}
      {spaces}
      {pinned}
      frame={IS_MAC ? "lights" : "controls"}
      onAction={act}
      onSpaceMenu={spaceMenu}
    />
  </RenderBoundary>
</div>
