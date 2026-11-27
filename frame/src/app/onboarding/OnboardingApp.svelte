<script lang="ts">
  import { installCloseService } from "$shared/lib/close";
  import { installChromeMenu } from "$shared/lib/chrome-menu";
  import "$styles/global.css";
  import { onMount, tick } from "svelte";
  import { commands } from "$shared/ipc/bindings";
  import { IS_MAC, IS_WINDOWS } from "$shared/platform";
  import { theme } from "$domain/appearance";
  import { operations } from "$domain/operations";
  import { preferences } from "$domain/preferences";
  import { tabs } from "$domain/tabs";
  import { browserImport, nativeImportAdapter } from "$domain/browser-import";
  import * as motion from "$session/motion.svelte";
  import { Onboarding } from "$features/onboarding";
  import WindowControls from "$shared/ui/WindowControls";
  import { applyLanguage } from "$shared/lib/locale.svelte";
  import { prepareSky, scene } from "$shared/sky";

  $effect(() => void applyLanguage(preferences.value("ui.language")));

  $effect(() => {
    theme.applyUiPreferences(
      preferences.value("ui.accent"),
      preferences.value("ui.reduce-motion") === "true",
    );
  });

  $effect(() => {
    theme.applyCustomization({
      theme: preferences.value("ui.theme"),
      custom: preferences.value("ui.theme-custom"),
      level: preferences.value("ui.theme-contrast"),
      contrast: preferences.value("ui.contrast") === "true",
      font: preferences.value("ui.font"),
      fontSize: preferences.value("ui.font-size"),
      codeFont: preferences.value("ui.code-font"),
    });
  });

  // Drawn once what it shows has arrived, and before native reveals the
  // window, so the first frame is already the welcome.
  let ready = $state(false);
  // Once native has taken the window for the browser, nothing of onboarding
  // stays running while its page is replaced.
  let finished = $state(false);

  async function finish(): Promise<boolean> {
    const opened = await commands.onboardingFinish().catch(() => false);
    if (opened) {
      finished = true;
      await tick();
    }
    return opened;
  }

  onMount(installCloseService);
  onMount(() => installChromeMenu());
  onMount(() => {
    let disposed = false;
    const themeReady = theme.init();
    const preferencesReady = preferences.init();
    void operations.init();
    // Bootstraps the shell behind this page, so a kept site or a name lands
    // in the session the browser opens with.
    const tabsReady = tabs.init();
    // An embedder (a test) may have provided its own port already.
    const ownsImport = !browserImport.available();
    if (ownsImport) browserImport.provide(nativeImportAdapter());

    void (async () => {
      try {
        await Promise.all([themeReady, preferencesReady, tabsReady]);
        // Welcome opens on the sky, so the window is shown with it whole.
        await Promise.race([
          prepareSky(scene()).catch(() => {}),
          new Promise((resolve) => setTimeout(resolve, 1500)),
        ]);
        if (disposed) return;
        ready = true;
        await tick();
        // A hidden native window can suspend animation frames indefinitely.
        // Resolve style/layout synchronously before admitting native reveal.
        void document.documentElement.getBoundingClientRect();
        void getComputedStyle(document.body).backgroundColor;
        if (!(await commands.uiReady())) {
          throw new Error("native startup gate rejected UI");
        }
        motion.reveal();
      } catch {
        console.error("trusted onboarding initialization failed");
      }
    })();

    return () => {
      disposed = true;
      theme.dispose();
      motion.dispose();
      preferences.dispose();
      operations.dispose();
      tabs.dispose();
      if (ownsImport) browserImport.provide(null);
    };
  });
</script>

{#if ready && !finished}
  <Onboarding onfinish={finish}
    >{#snippet windowControls()}{#if !IS_MAC && !IS_WINDOWS}<WindowControls
        />{/if}{/snippet}</Onboarding
  >
{/if}
