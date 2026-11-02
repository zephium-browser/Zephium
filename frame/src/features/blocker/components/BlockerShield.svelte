<script lang="ts">
  import { untrack, type Snippet } from "svelte";
  import * as m from "$shared/i18n/messages";
  import { blocker, shieldPresentation } from "$domain/blocker";

  /** The blocking section of the site panel. Its controls load the first time
   *  the panel is open, a failed load is tried again at the next, and the
   *  standing is read afresh at every opening. `head` draws the section's first
   *  row, whose mark is the page's own: given what the switch is called, and
   *  the switch once there is one. It is drawn without a name when blocking is
   *  absent, so the row is there whatever blocking is. */
  let { open, head }: { open: boolean; head?: Snippet<[name: string | null, control?: Snippet]> } =
    $props();
  let Menu = $state<typeof import("./ProtectionMenu.svelte").default | null>(null);
  let failed = $state(false);
  let loading = false;

  $effect(() => {
    if (!open) return;
    untrack(() => {
      void blocker.refresh();
      if (Menu !== null || loading) return;
      loading = true;
      failed = false;
      void import("./ProtectionMenu.svelte")
        .then((module) => (Menu = module.default))
        .catch(() => (failed = true))
        .finally(() => (loading = false));
    });
  });
  let shield = $derived(shieldPresentation(blocker.status()));
</script>

{#if Menu && shield.visible}
  <Menu {head} />
{:else if head || (failed && shield.visible)}
  <section class="protection" data-tray-tools>
    {#if head}{@render head(shield.visible ? m.site_info_blocking() : null)}{/if}
    {#if failed}<p class="failure" role="status">{m.surface_render_failed()}</p>{/if}
  </section>
{/if}

<style>
  .failure {
    margin: 0 var(--menu-item-inset) 4px;
    color: var(--color-warning);
    font-size: var(--text-caption);
  }
</style>
