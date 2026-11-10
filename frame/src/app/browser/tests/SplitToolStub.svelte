<script lang="ts">
  let { tool }: { tool: string } = $props();
</script>

<input aria-label="Tool draft" data-tool={tool} />
