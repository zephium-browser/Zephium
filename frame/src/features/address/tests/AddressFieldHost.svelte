<script lang="ts">
  import AddressField from "../components/AddressField.svelte";

  let { compact = false, sidebarVisible = true }: { compact?: boolean; sidebarVisible?: boolean } =
    $props();
</script>

<AddressField {compact} {sidebarVisible}>
  {#snippet trailing(page)}
    <output data-trailing>{JSON.stringify(page)}</output>
  {/snippet}
</AddressField>
