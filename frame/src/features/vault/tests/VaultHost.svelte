<script lang="ts">
  import VaultPage from "../components/VaultPage.svelte";
  let { profile }: { profile: string } = $props();
</script>

<div class="stage">
  <div class="rail"></div>
  <div class="page"><VaultPage {profile} onclose={() => {}} /></div>
</div>

<style>
  .stage {
    display: flex;
    height: 940px;
    padding: 8px 8px 8px 0;
    box-sizing: border-box;
    background: var(--color-chrome);
  }

  /* Stands in for the browser's tab rail beside the page. */
  .rail {
    flex: 0 0 56px;
  }

  .page {
    flex: 1;
    min-width: 0;
  }
</style>
