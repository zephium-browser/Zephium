<script lang="ts">
  import type { Snippet } from "svelte";
  import { ShieldAlertIcon, ShieldCheckIcon } from "@hugeicons/core-free-icons";
  import * as m from "$shared/i18n/messages";
  import Icon from "$shared/ui/Icon";
  import type { SitePage } from "../lib/address-model";

  /** The first row of the panel: a shield that says whether the connection can
   *  be trusted, what the control at the row's end does, and the control. A
   *  secure page says so on its shield alone; one that is not says why in words
   *  under the name. With no `label` the status itself takes the name's place. */
  let {
    page,
    label = null,
    children,
  }: { page: SitePage | null; label?: string | null; children?: Snippet } = $props();

  let secure = $derived(page?.security === "secure");
  let status = $derived(
    page?.security === "untrusted"
      ? m.site_info_untrusted()
      : page?.security === "insecure"
        ? m.site_info_insecure()
        : m.site_info_secure(),
  );
</script>

{#if page}
  <div class="head">
    <span
      class="mark"
      role="img"
      aria-label={status}
      title={status}
      data-secure={secure || undefined}
    >
      <Icon icon={secure ? ShieldCheckIcon : ShieldAlertIcon} size={16} />
    </span>
    <span class="words">
      <span class="name">{label ?? status}</span>
      {#if label && !secure}<span class="status">{status}</span>{/if}
    </span>
    {@render children?.()}
  </div>
{/if}

<style>
  .head {
    display: flex;
    align-items: center;
    gap: 10px;
    min-block-size: var(--menu-row);
    padding: 2px var(--menu-item-inset);
  }

  .mark {
    display: grid;
    flex: none;
    place-items: center;
    inline-size: 16px;
    block-size: 16px;
    color: var(--color-danger);
  }

  .mark[data-secure] {
    color: var(--color-success);
  }

  .words {
    display: grid;
    flex: 1;
    min-inline-size: 0;
  }

  .name {
    color: var(--color-text);
    font-size: var(--text-body);
    font-weight: 450;
    line-height: 16px;
    overflow-wrap: break-word;
    text-wrap: balance;
  }

  .status {
    color: var(--color-danger);
    font-size: var(--text-caption);
    line-height: 14px;
  }
</style>
