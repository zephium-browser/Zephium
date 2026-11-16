<script lang="ts">
  import Sidebar from "../components/Sidebar.svelte";

  let { panel = "" }: { panel?: string } = $props();
</script>

<Sidebar>
  {#snippet settingsNavigation()}{/snippet}
  {#snippet toolPanel()}<input aria-label="Tool field" />{/snippet}
  {#snippet dock()}{/snippet}
  {#snippet browserBody()}
    <ul>
      <!-- A live Essential's data-open marks its page, not an open panel. -->
      <li class="essential" data-open="true"><button type="button">Live Essential</button></li>
    </ul>
    <input aria-label="Sidebar field" />
    <!-- An explicit tab stop keeps the fixture independent of host keyboard navigation settings. -->
    <button type="button" tabindex="0">Sidebar control</button>
    {#if panel}<div class={panel} data-open="true"></div>{/if}
  {/snippet}
</Sidebar>
