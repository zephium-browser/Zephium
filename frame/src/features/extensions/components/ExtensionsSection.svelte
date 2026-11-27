<script lang="ts">
  import { extensions } from "$domain/extensions";
  import { tabs } from "$domain/tabs";
  import { webext } from "$domain/webext";
  import ExtensionActions from "./ExtensionActions.svelte";
  import ManageExtensions from "./ManageExtensions.svelte";

  let failure = $derived(extensions.failureReason(tabs.profile()?.id ?? null, tabs.activeId()));

  const failureMessage = (reason: ReturnType<typeof extensions.failureReason>) => {
    switch (reason) {
      case "tab_discarded":
        return "This tab is sleeping. Activate it before opening the extension.";
      case "action_disabled":
        return "This extension action is disabled on the current page.";
      case "popup_capacity_exceeded":
      case "capacity_exceeded":
        return "Close the current extension popup and try again.";
      case "unsupported_platform":
        return "This extension action is not supported on this platform.";
      case "shutting_down":
        return "Extensions are unavailable while Zephium is closing.";
      case "runtime_superseded":
      case "runtime_unavailable":
      case "tab_unavailable":
      case "action_unavailable":
        return "The extension changed. Try the action again.";
      case "popup_unavailable":
        return "This extension popup is unavailable.";
      case "invalid_request":
      case "native_admission_failed":
        return "Zephium couldn't open this extension action.";
      case null:
        return "";
    }
  };
</script>

<!-- Stays mounted while the panel is closed: an extension's key opens its
     action from the buttons here. The name and the way to manage them, then
     a row for each extension. -->
{#if webext.isAvailable()}
  <div class="extensions" data-tray-tools>
    <div class="row">
      <ManageExtensions />
    </div>
    <ExtensionActions />
    {#if failure !== null}
      <p role="status" aria-live="polite" class="failure">{failureMessage(failure)}</p>
    {/if}
  </div>
{/if}

<style>
  /* The name's text starts where the icons of the rows around do, and the
     buttons' glyphs end where their switch does: their plates reach past. */
  .row {
    display: flex;
    align-items: center;
    gap: 8px;
    min-block-size: var(--menu-row);
    padding-block: 2px;
    padding-inline: 0 calc(var(--menu-item-inset) - 4px);
  }

  .failure {
    margin-block: 0 4px;
    margin-inline: var(--menu-item-inset);
    font-size: var(--text-label);
    line-height: 1.4;
    color: var(--color-muted);
  }
</style>
