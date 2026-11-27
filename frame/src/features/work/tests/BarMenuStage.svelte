<script lang="ts">
  import { Popover } from "bits-ui";
  import WorkBar from "../components/bar/WorkBar.svelte";

  let { restoreFocus = true }: { restoreFocus?: boolean } = $props();
  let value = $state("");
  let open = $state(false);
</script>

<div class="stage">
  <WorkBar bind:value placeholder="What do you want to do?" onsubmit={() => {}}>
    {#snippet trailing()}
      <Popover.Root bind:open>
        <Popover.Trigger class="model-trigger">Model</Popover.Trigger>
        <Popover.Portal>
          <Popover.Content
            onCloseAutoFocus={(event) => {
              if (!restoreFocus) event.preventDefault();
            }}
          >
            <button type="button" onclick={() => (open = false)}>Opus</button>
          </Popover.Content>
        </Popover.Portal>
      </Popover.Root>
    {/snippet}
  </WorkBar>
</div>

<style>
  .stage {
    display: flex;
    justify-content: center;
    padding-block-start: 300px;
  }
</style>
