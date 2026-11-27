<script lang="ts">
  import AccountDialog from "../components/account/AccountDialog.svelte";
  let open = $state(true);
  let pending = $state(true);
  let failed = $state(false);
</script>

<AccountDialog
  bind:open={
    () => open,
    (next) => {
      if (!pending) open = next;
    }
  }
  title="Clear site data"
>
  {#if failed}<p role="alert">Clear failed. Try again.</p>{/if}
  <button
    type="button"
    onclick={() => {
      pending = false;
      failed = true;
    }}>Finish with failure</button
  >
</AccountDialog>
