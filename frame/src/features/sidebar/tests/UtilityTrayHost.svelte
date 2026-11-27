<script lang="ts">
  import UtilityTray from "../components/UtilityTray.svelte";

  let { warning = null }: { warning?: string | null } = $props();
</script>

<UtilityTray label="Site information" {warning}>
  {#snippet children(open)}
    <section aria-label="Section">
      <button type="button">Inside</button>
      <output>{open ? "showing" : "hidden"}</output>
    </section>
  {/snippet}
</UtilityTray>
