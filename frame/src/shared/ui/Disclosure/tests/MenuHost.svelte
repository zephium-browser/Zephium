<script lang="ts">
  import Disclosure from "../Disclosure.svelte";
</script>

<Disclosure label="Actions" menu>
  {#snippet trigger()}Actions{/snippet}
  <button type="button" role="menuitem">First</button>
  <button type="button" role="menuitem" disabled>Unavailable</button>
  <button type="button" role="menuitem">Last</button>
</Disclosure>
