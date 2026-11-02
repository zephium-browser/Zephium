<script lang="ts">
  import * as m from "$shared/i18n/messages";
  import type { SitePage } from "../lib/address-model";

  /** What the panel says of the page beyond its connection, loaded the first
   *  time the panel is opened; a failed load is tried again at the next. */
  let { page, open }: { page: SitePage | null; open: boolean } = $props();

  let Body = $state.raw<typeof import("./SiteDetailsBody.svelte").default | null>(null);
  let failed = $state(false);
  let loading = false;

  $effect(() => {
    if (!open || Body !== null || loading) return;
    loading = true;
    failed = false;
    void import("./SiteDetailsBody.svelte")
      .then((module) => (Body = module.default))
      .catch(() => (failed = true))
      .finally(() => (loading = false));
  });
</script>

{#if page && Body}
  <Body {page} {open} />
{:else if page && failed}
  <p role="status">{m.surface_render_failed()}</p>
{/if}

<style>
  p {
    margin: 0;
    padding-inline: var(--menu-item-inset);
    color: var(--color-muted);
    font-size: var(--text-label);
  }
</style>
