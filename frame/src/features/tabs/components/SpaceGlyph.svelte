<script lang="ts">
  import type { SpaceIconView } from "$shared/ipc/bindings";
  import IconMark from "$shared/ui/IconMark";

  let {
    icon,
    color,
    size = 16,
  }: {
    icon: SpaceIconView | null;
    color: number;
    size?: number;
  } = $props();
</script>

<!-- A space's mark is the same glyph a person can put on an Essential. -->
<IconMark {icon} {color} {size} />
