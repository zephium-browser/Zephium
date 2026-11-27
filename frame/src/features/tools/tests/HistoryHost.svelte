<script lang="ts">
  import ToolSlot from "../components/ToolSlot.svelte";
  let { profile }: { profile: string } = $props();
</script>

<div class="host" style:width="384px">
  <div class="sidebar-columns">
    <div class="sidebar-tool-host">
      <ToolSlot tool="history" {profile} onclose={() => {}} />
    </div>
  </div>
</div>

<style>
  .host {
    font: 13px system-ui;
    color: var(--color-text);
    background: var(--color-chrome);
    display: flex;
    height: 700px;
    min-width: 0;
  }
</style>
