<script lang="ts">
  import { Add01Icon, CookieIcon, MinusSignIcon, ZoomInAreaIcon } from "@hugeicons/core-free-icons";
  import * as m from "$shared/i18n/messages";
  import { clearRequest, pageSite } from "$domain/site-data";
  import { commands } from "$shared/ipc/bindings";
  import Button from "$shared/ui/Button";
  import Icon from "$shared/ui/Icon";
  import IconButton from "$shared/ui/IconButton";
  import type { SitePage } from "../lib/address-model";
  import SitePermissions from "./SitePermissions.svelte";

  /** The decisions the site has, how far the page is zoomed, and its data:
   *  one section of the panel, whatever of them there is. */
  let { page, open }: { page: SitePage; open: boolean } = $props();

  let site = $derived(pageSite(page.url));
  let zoomed = $derived(page.zoom !== null && page.zoom !== page.standardZoom);
  // The row that stepped the page back to the default stays until the panel
  // closes, so the control pressed is not taken from under the pointer.
  let stayed = $state(false);
  $effect(() => {
    if (!open) stayed = false;
    else if (zoomed) stayed = true;
  });

  const step = (command: string) => void commands.runCommand(command);
</script>

<div class="details">
  {#if site && !page.privateWindow}
    <SitePermissions origin={site.origin} {open} capture={page.capture} />
  {/if}

  {#if zoomed || stayed}
    <div class="zoom" role="group" aria-label={m.site_info_zoom()}>
      <span class="ui-menu-icon" aria-hidden="true" title={m.site_info_zoom()}
        ><Icon icon={ZoomInAreaIcon} size={16} /></span
      >
      <output aria-live="polite"
        >{m.zoom_percent({ percent: page.zoom ?? page.standardZoom })}</output
      >
      <div class="controls">
        <IconButton
          icon={MinusSignIcon}
          size={14}
          buttonSize={26}
          label={m.command_zoom_out()}
          onclick={() => step("zoom.out")}
        />
        <IconButton
          icon={Add01Icon}
          size={14}
          buttonSize={26}
          label={m.command_zoom_in()}
          onclick={() => step("zoom.in")}
        />
        <Button size="compact" onclick={() => step("zoom.reset")}>{m.zoom_reset()}</Button>
      </div>
    </div>
  {/if}

  {#if !page.privateWindow}
    <button
      type="button"
      class="ui-menu-item row"
      data-dismiss
      onclick={() => clearRequest.ask(page.url)}
    >
      <span class="ui-menu-icon" aria-hidden="true"><Icon icon={CookieIcon} size={16} /></span>
      <span class="label">{m.site_info_clear_data()}</span>
    </button>
  {/if}
</div>

<style>
  .details {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-inline-size: 0;
  }

  .details:not(:has(*)) {
    display: none;
  }

  /* Its level is the name: the icon says what it is the level of. */
  .zoom {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 2px 10px;
    min-block-size: var(--menu-row);
    padding-inline: var(--menu-item-inset);
  }

  output {
    flex: 1 0 auto;
    color: var(--color-text);
    font-size: var(--text-body);
    font-variant-numeric: tabular-nums;
    font-weight: 450;
  }

  .controls {
    display: flex;
    align-items: center;
    gap: 4px;
    margin-inline-start: auto;
  }

  .row {
    inline-size: 100%;
    border: 0;
    background: transparent;
    font: inherit;
    font-size: var(--text-body);
    font-weight: 450;
    text-align: start;
  }

  .row:focus-visible,
  .row:hover {
    background: var(--row-active);
  }

  .row:focus-visible {
    outline: 2px solid var(--color-ring);
    outline-offset: -2px;
  }

  .row:is(:hover, :focus-visible) .ui-menu-icon {
    color: var(--color-text);
  }

  .label {
    flex: 1;
    min-inline-size: 0;
    text-wrap: balance;
  }
</style>
