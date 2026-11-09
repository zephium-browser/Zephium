<script lang="ts">
  import RenderBoundary from "$shared/ui/RenderBoundary";
  import "$styles/little.css";
  import { onMount, flushSync, tick } from "svelte";
  import type { LittleIntent, TabView } from "$shared/ipc/bindings";
  import { commands } from "$shared/ipc/bindings";
  import { events } from "$shared/ipc/native-events";
  import { theme } from "$domain/appearance";
  import {
    acceptLittleTab,
    acceptLittleView,
    LittleToolbar,
    windowOfLabel,
  } from "$features/little";
  import * as m from "$shared/i18n/messages";
  import { applyLanguage } from "$shared/lib/locale.svelte";

  let { label }: { label: string } = $props();
  // Native names the window by this label; it never changes.
  const id = $derived(windowOfLabel(label));

  let tab = $state<TabView | null>(null);
  let spaces = $state(0);

  // The presentation barrier reads this DOM back in the same turn.
  function show(next: TabView | null) {
    flushSync(() => {
      tab = next;
    });
  }
  function act(intent: LittleIntent) {
    void commands.littleAction(intent).catch(() => {});
  }
  function spaceMenu(x: number, y: number) {
    void commands.littleSpaceMenu(x, y).catch(() => {});
  }
  // The page usually holds focus; the menu bar routes Cmd+W, Cmd+L and the
  // page keys to this window then. Cmd+O works while the toolbar holds it.
  function onkeydown(event: KeyboardEvent) {
    if (!event.metaKey || event.altKey || event.ctrlKey || event.shiftKey) return;
    if (event.key.toLowerCase() !== "o" || !tab) return;
    event.preventDefault();
    act("open_in_main");
  }

  onMount(() => {
    let disposed = false;
    let stopView: (() => void) | undefined;
    let stopPresentation: (() => void) | undefined;
    let stopCommands: (() => void) | undefined;
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
    const viewListener = events.little.listen(({ payload }) => {
      if (disposed || id === null || payload.window !== id) return;
      spaces = payload.spaces.length;
      if (acceptLittleView(id, tab, payload)) show(payload.tab);
    });
    const presentationListener = events.littlePresentation.listen(({ payload }) => {
      if (!disposed && acceptLittleTab(tab, payload.tab)) show(payload.tab);
    });

    void (async () => {
      try {
        stopView = await viewListener;
        stopPresentation = await presentationListener;
        stopCommands = await uiListener;
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
        await commands.littleReady();
      } catch {
        // The window stays hidden; closing the app removes it.
      }
    })();
    return () => {
      disposed = true;
      stopView?.();
      stopPresentation?.();
      stopCommands?.();
      theme.dispose();
    };
  });
</script>

<svelte:window {onkeydown} />

<div class="little-root">
  <RenderBoundary title={m.surface_render_failed()} retryLabel={m.surface_retry()}>
    <LittleToolbar {tab} {spaces} onAction={act} onSpaceMenu={spaceMenu} />
  </RenderBoundary>
</div>
