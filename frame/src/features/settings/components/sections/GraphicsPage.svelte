<script lang="ts">
  import { onDestroy } from "svelte";
  import * as m from "$shared/i18n/messages";
  import SettingsGroup from "$shared/ui/SettingsGroup";
  import RenderingMode from "../graphics/RenderingMode.svelte";
  import GraphicsDetails from "../graphics/GraphicsDetails.svelte";
  import { createGraphics } from "../../lib/graphics-state.svelte";

  const graphics = createGraphics();
  onDestroy(graphics.dispose);
</script>

<SettingsGroup title={m.graphics_mode()} description={m.graphics_mode_help()}>
  <RenderingMode {graphics} />
</SettingsGroup>
<SettingsGroup title={m.graphics_details()} description={m.graphics_details_note()}>
  <GraphicsDetails {graphics} />
</SettingsGroup>
<span class="sr-only" role="status">{graphics.announcement()}</span>
