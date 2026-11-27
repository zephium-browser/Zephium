<script lang="ts">
  import AddressField from "../components/AddressField.svelte";

  let { compact = false }: { compact?: boolean } = $props();
</script>

<AddressField {compact}>
  {#snippet trailing(page)}
    <output data-trailing>{JSON.stringify(page)}</output>
  {/snippet}
</AddressField>
