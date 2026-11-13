<script lang="ts">
  import "$styles/global.css";
  import type { ToolKind } from "$shared/ipc/bindings";
  import ToolSlot from "../components/ToolSlot.svelte";
  let { tool }: { tool: ToolKind } = $props();
</script>

<!-- A tool panel as the column hosts it, over the chrome's own ground. -->
<div class="host">
  <div class="sidebar-columns">
    <div class="sidebar-tool-host">
      <ToolSlot {tool} profile="p" onclose={() => {}} />
    </div>
  </div>
</div>

<style>
  .host {
    display: flex;
    width: 380px;
    height: 640px;
    min-width: 0;
    background: var(--color-chrome);
    color: var(--color-text);
  }
</style>
