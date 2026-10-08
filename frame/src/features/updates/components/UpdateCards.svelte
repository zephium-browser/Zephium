<!--
  The foot of the full-width column: a notice card, then the update waiting
  on a relaunch, both just above the dock.
-->
<script lang="ts">
  import * as m from "$shared/i18n/messages";
  import SidebarCard from "$shared/ui/SidebarCard";
  import * as notices from "../lib/notices.svelte";
  import { PILL_ICON, activatePill, cardView, pillLabel } from "../lib/present";

  let selected = $derived(notices.current());
  let card = $derived(selected.card ? cardView(selected.card) : null);
  let pill = $derived(selected.pill);
</script>

{#if card || pill}
  <div class="update-stack">
    {#if card}
      {#key card.key}<SidebarCard
          title={card.title}
          detail={card.detail}
          items={card.items}
          actions={card.actions}
          dismissLabel={m.update_dismiss()}
          ondismiss={card.dismiss}
        />{/key}
    {/if}
    {#if pill}
      <SidebarCard
        variant="pill"
        title={pillLabel(pill)}
        icon={PILL_ICON}
        pending={pill.kind === "installing"}
        onclick={() => {
          if (pill) activatePill(pill);
        }}
      />
    {/if}
  </div>
{/if}

<style>
  .update-stack {
    display: flex;
    flex: none;
    flex-direction: column;
    gap: 6px;
    margin: 6px 8px 0;
  }
</style>
