<script lang="ts">
  import { installCloseService } from "$shared/lib/close";
  import { installChromeMenu } from "$shared/lib/chrome-menu";
  import RenderBoundary from "$shared/ui/RenderBoundary";
  import "$styles/panel.css";
  import { onMount, flushSync, tick } from "svelte";
  import type { PanelState, PanelIntent, ToolKind } from "$shared/ipc/bindings";
  import { commands } from "$shared/ipc/bindings";
  import { events } from "$shared/ipc/native-events";
  import { theme } from "$domain/appearance";
  import { favicons } from "$domain/favicons";
  import { acceptPanelState } from "$features/panel";
  import { loadLauncherPanel } from "$features/search";
  import { loadCapture } from "$features/tasks";
  import * as m from "$shared/i18n/messages";
  import { applyLanguage } from "$shared/lib/locale.svelte";

  let presentation = $state<PanelState | null>(null);
  let Launcher = $state<Awaited<ReturnType<typeof loadLauncherPanel>>["default"] | null>(null);
  let failed = $state(false);
  let captures = $state(0);
  $effect(() => {
    const state = presentation;
    if (!state || state.visible || captures !== 0) return;
    const timer = setTimeout(
      () => void commands.panelIntent({ type: "idle", revision: state.revision }).catch(() => {}),
      1000,
    );
    return () => clearTimeout(timer);
  });
  function apply(next: PanelState) {
    if (acceptPanelState(presentation, next))
      flushSync(() => {
        presentation = next;
        failed = false;
      });
  }
  async function intent(value: PanelIntent) {
    try {
      if (!(await commands.panelIntent(value))) failed = true;
    } catch {
      failed = true;
    }
  }
  onMount(installCloseService);
  onMount(() => installChromeMenu());
  onMount(() => {
    let disposed = false;
    let stop: (() => void) | undefined;
    let stopMotion: (() => void) | undefined;
    let liveMotion = false;
    let liveLanguage = false;
    const liveFeatures: string[] = [];
    const liveLook: string[] = [];
    const motionListener = events.uiCommand.listen(({ payload }) => {
      if (disposed) return;
      const look = theme.customizationKeys.find((key) => payload.startsWith(`preference.${key}=`));
      if (look) {
        liveLook.push(look);
        theme.customize(look, payload.slice(`preference.${look}=`.length));
      }
      const feature = FEATURES.find((key) => payload.startsWith(`preference.${key}.enabled=`));
      if (feature) {
        liveFeatures.push(feature);
        setFeature(feature, payload.endsWith("=false"));
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
    const listener = events.panelState.listen((event) => {
      if (!disposed) apply(event.payload);
    });

    // Result rasters arrive just ahead of the results that reference them.
    const faviconsReady = favicons.init();
    void (async () => {
      try {
        // Listening comes first, so a setting changed while the reads below
        // are in flight is heard rather than lost between read and listener.
        [stop, stopMotion] = await Promise.all([listener, motionListener]);
        // The launcher is loaded while the window is still hidden, and native
        // shows nothing before `panelReady`, so the first frame is never empty.
        // Nothing below waits on anything else, so the wait is the slowest one.
        const [launcher, , , [motion, language, notesOn, tasksOn, ...look]] = await Promise.all([
          loadLauncherPanel(),
          theme.init(),
          faviconsReady,
          Promise.all([
            commands.settingGet("ui.reduce-motion").catch(() => null),
            commands.settingGet("ui.language").catch(() => null),
            commands.settingGet("notes.enabled").catch(() => null),
            commands.settingGet("tasks.enabled").catch(() => null),
            ...theme.customizationKeys.map((key) => commands.settingGet(key).catch(() => null)),
          ]),
        ]);
        if (disposed) return;
        Launcher = launcher.default;
        theme.customizationKeys.forEach((key, index) => {
          const value = look[index];
          if (!liveLook.includes(key) && value != null) theme.customize(key, value);
        });
        if (!liveMotion) theme.setReducedMotion(motion === "true");
        if (!liveLanguage) void applyLanguage(language);
        if (!liveFeatures.includes("notes")) setFeature("notes", notesOn === "false");
        if (!liveFeatures.includes("tasks")) setFeature("tasks", tasksOn === "false");
        await tick();
        if (disposed) return;
        void document.documentElement.getBoundingClientRect();
        const initial = await commands.panelReady();
        if (!disposed) {
          if (initial) apply(initial);
          else failed = true;
        }
      } catch {
        if (!disposed) failed = true;
      }
    })();
    return () => {
      disposed = true;
      stop?.();
      stopMotion?.();
      favicons.dispose();
      theme.dispose();
    };
  });
  /** The launcher offers no way into a feature switched off. */
  const FEATURES = ["notes", "tasks"] as const;
  let off = $state<string[]>([]);
  function setFeature(feature: string, isOff: boolean) {
    off = isOff
      ? [...off.filter((key) => key !== feature), feature]
      : off.filter((key) => key !== feature);
  }
  function open(kind: ToolKind) {
    void intent({ type: "open", tool: kind });
  }
  /** Long enough to read that it landed, short enough not to wait on. */
  const CAPTURED_MS = 700;
  async function capture(text: string): Promise<string | null> {
    const profile = presentation?.profile_id;
    if (!profile) return null;
    const session = presentation?.session_id;
    captures += 1;
    try {
      const { captureTask } = await loadCapture();
      const title = await captureTask(profile, text);
      if (title)
        setTimeout(() => {
          if (presentation?.visible && presentation.session_id === session)
            void intent({ type: "dismiss" });
        }, CAPTURED_MS);
      return title;
    } finally {
      captures -= 1;
    }
  }
</script>

<div
  class="panel-root"
  style:--panel-radius={`${presentation?.corner_radius ?? 20}px`}
  data-visible={presentation?.visible ?? false}
>
  {#if failed}<div class="panel-error" role="alert">{m.panel_action_failed()}</div>{/if}
  <RenderBoundary title={m.surface_render_failed()} retryLabel={m.surface_retry()}>
    {#if Launcher}{#key off.join()}<Launcher
          context={presentation}
          onTool={open}
          onCapture={off.includes("tasks") ? undefined : capture}
          hidden={off}
          onDismiss={() => void intent({ type: "dismiss" })}
        />{/key}{/if}
  </RenderBoundary>
</div>
