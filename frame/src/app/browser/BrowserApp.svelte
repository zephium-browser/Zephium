<script lang="ts">
  import { installCloseService } from "$shared/lib/close";
  import "$styles/global.css";
  import { onMount, untrack } from "svelte";
  import { WebExtensionAccessPrompt, WebExtensionReview } from "$features/webext";
  import { webext } from "$domain/webext";
  import { PagePermissionPrompt } from "$features/permissions";
  import { updateNotices } from "$features/updates";
  import * as sidebar from "$session/sidebar-mode.svelte";
  import { Dividers } from "$features/split";
  import { commands } from "$shared/ipc/bindings";
  import { IS_MAC, IS_WINDOWS } from "$shared/platform";
  import { blocker } from "$domain/blocker";
  import { extensions } from "$domain/extensions";
  import { favicons } from "$domain/favicons";
  import { layout } from "$domain/layout";
  import { operations } from "$domain/operations";
  import { pagePermissions } from "$domain/permissions";
  import { runtime } from "$domain/runtime";
  import { updates } from "$domain/updates";
  import { focus } from "$domain/time";
  import { tabs } from "$domain/tabs";
  import { theme } from "$domain/appearance";
  import { uiCommands as ui } from "$domain/ui-commands";
  import { surface as browserPage } from "$domain/surface";
  import * as motion from "$session/motion.svelte";
  import * as tools from "$session/tools.svelte";
  import { preferences } from "$domain/preferences";
  import { keymap } from "$domain/keymap";
  import { browserImport, nativeImportAdapter } from "$domain/browser-import";
  import { findInPage } from "$features/address";
  import { keymapAcceleratorFrom } from "$shared/lib/accelerator";
  import { applyLanguage } from "$shared/lib/locale.svelte";
  import Shell from "./Shell.svelte";

  // Follows the stored preference and nothing else. Adopting reads the
  // sidebar's own shape; tracked, that read made every toggle re-adopt the
  // not-yet-saved old value, bouncing the column back and forth and cutting
  // the page's slide short.
  $effect(() => {
    const mode = preferences.value("sidebar.mode");
    if (mode === "default" || mode === "compact") untrack(() => sidebar.adoptMode(mode));
  });

  $effect(() => {
    theme.applyUiPreferences(
      preferences.value("ui.accent"),
      preferences.value("ui.reduce-motion") === "true",
    );
  });

  $effect(() => applyLanguage(preferences.value("ui.language")));

  $effect(() => {
    const text = preferences.value("ui.text-size");
    theme.applyLayout({
      density: preferences.value("ui.density") === "compact" ? "compact" : "comfortable",
      contrast: preferences.value("ui.contrast") === "true",
      text: text === "large" || text === "small" ? text : "default",
    });
  });

  let pagePermissionPrompt = $derived(pagePermissions.prompt());
  let consentActive = $derived(
    pagePermissionPrompt !== null || webext.review() !== null || webext.accessRequest() !== null,
  );

  // Page keystrokes are matched natively (menu bar, engine key tables, the
  // GTK window handler); these are the same keys while chrome has focus,
  // read from the one resolved keymap.
  function handleKeydown(event: KeyboardEvent) {
    // The native content stage is suppressed while this browser-owned modal
    // is active. Keep chrome shortcuts from mutating tabs behind it as well.
    if (consentActive || event.defaultPrevented || event.isComposing) return;
    const pressed = keymapAcceleratorFrom(event, IS_MAC, IS_WINDOWS);
    if (pressed === null) return;
    const command = keymap.commandFor(event, IS_MAC, pressed);
    if (command === null) return;
    event.preventDefault();
    void commands.runCommand(command);
  }

  onMount(installCloseService);
  // The startup read can land before the session has a profile, which lists
  // nothing; read again once the profile is known, and when it changes.
  let extensionProfile = $derived(tabs.profile()?.id);
  $effect(() => {
    if (extensionProfile) void webext.refresh();
  });
  onMount(() => {
    let disposed = false;

    // Each async initializer performs its synchronous setup before its first
    // await. In particular, tabs installs every scoped projection listener
    // before bootstrap can ask native code for a snapshot.
    const themeReady = theme.init();

    // Runtime status is emitted by the same actor-ordered bootstrap that
    // supplies tabs, so its listener must exist before tabs starts bootstrap.
    const toolsReady = tools.init();
    const browserPageReady = browserPage.init();
    const preferencesReady = preferences.init();
    const runtimeReady = runtime.init();
    void focus.init();
    void updates.init();
    void updateNotices.init();
    const extensionsReady = extensions.init();
    void webext.refresh();
    const stopAccess = webext.listenForAccess();
    const stopDrops = webext.listenForDrops(() => browserPage.currentPage() !== "work");
    const pagePermissionsReady = pagePermissions.init();

    // Rasters are emitted immediately before the projection that references
    // them, so this listener must exist before tabs asks native to bootstrap.
    const faviconsReady = favicons.init();
    const tabsReady = tabs.init();
    const sidebarReady = sidebar.init();
    const uiEventsReady = ui.init();
    void keymap.init();
    // An embedder (a test) may have provided its own port already.
    const ownsImport = !browserImport.available();
    if (ownsImport) browserImport.provide(nativeImportAdapter());
    void operations.init();
    void blocker.init();
    if (!IS_MAC) void layout.init();
    document.addEventListener("keydown", handleKeydown);

    void (async () => {
      try {
        await Promise.all([
          toolsReady,
          browserPageReady,
          preferencesReady,
          themeReady,
          runtimeReady,
          extensionsReady,
          pagePermissionsReady,
          faviconsReady,
          tabsReady,
          uiEventsReady,
          sidebarReady,
        ]);
        if (disposed) return;

        // A hidden native window can suspend animation frames indefinitely.
        // Resolve style/layout synchronously before admitting native reveal.
        void document.documentElement.getBoundingClientRect();
        void getComputedStyle(document.body).backgroundColor;
        if (!(await commands.uiReady())) {
          throw new Error("native startup gate rejected UI");
        }
        motion.reveal();
      } catch {
        // Native owns the fail-closed watchdog. Keep attacker-controlled page
        // data and native internals out of this static diagnostic.
        console.error("trusted UI initialization failed");
      }
    })();

    return () => {
      disposed = true;
      theme.dispose();
      browserPage.dispose();
      tools.dispose();
      motion.dispose();
      preferences.dispose();
      operations.dispose();
      blocker.dispose();
      runtime.dispose();
      updates.dispose();
      updateNotices.dispose();
      focus.dispose();
      extensions.dispose();
      pagePermissions.dispose();
      void stopAccess.then((stop) => stop());
      void stopDrops.then((stop) => stop());
      favicons.dispose();
      tabs.dispose();
      ui.dispose();
      keymap.dispose();
      findInPage.dispose();
      if (ownsImport) browserImport.provide(null);
      if (!IS_MAC) layout.dispose();
      document.removeEventListener("keydown", handleKeydown);
    };
  });
</script>

<div class="contents" inert={consentActive}>
  <Shell />
  {#if !IS_MAC}
    <Dividers />
  {/if}
</div>
{#if webext.review() !== null}
  <WebExtensionReview review={webext.review()!} />
{:else if webext.accessRequest() !== null}
  {#key webext.accessRequest()!.request}
    <WebExtensionAccessPrompt request={webext.accessRequest()!} />
  {/key}
{:else if pagePermissionPrompt !== null}
  {#key `${pagePermissionPrompt.profile_id}:${pagePermissionPrompt.item_id}:${pagePermissionPrompt.request_id}`}
    <PagePermissionPrompt prompt={pagePermissionPrompt} />
  {/key}
{/if}
