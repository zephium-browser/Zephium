<script lang="ts">
  import { useSvelteFlow } from "@xyflow/svelte";
  type Flow = ReturnType<typeof useSvelteFlow>;
  /** Hands the flow's helpers to the canvas, which sits outside the flow's context. */
  let { onready }: { onready: (flow: Flow) => void } = $props();
  const flow = useSvelteFlow();
  $effect(() => onready(flow));
</script>
