<script lang="ts">
  import { Settings01Icon } from "@hugeicons/core-free-icons";
  import type { ToolHostProps } from "$session/tool-drafts.svelte";
  import { commands } from "$shared/ipc/bindings";
  import { surface as browser } from "$domain/surface";
  import { tabs } from "$domain/tabs";
  import { loadTimePanel } from "$features/time";
  import LazyView from "$shared/ui/LazyView";
  import type { MenuEntry } from "$shared/ui/Menu";
  import * as m from "$shared/i18n/messages";
  import ToolFrame from "../ToolFrame.svelte";
  let props: ToolHostProps = $props();

  const entries: MenuEntry[] = [
    { kind: "item", id: "settings", label: m.time_settings(), icon: Settings01Icon },
  ];

  // The page is the same time in more depth, so the panel does not stay open
  // behind it.
  function expand() {
    void browser.open("time");
    props.onclose();
  }

  function act(id: string) {
    if (id === "settings") {
      void commands.runCommand("settings.focus");
      props.onclose();
    }
  }
</script>

<ToolFrame
  {...props}
  caption={false}
  onexpand={expand}
  more={entries}
  onmore={act}
  filters={[
    { value: "today", label: m.tool_today() },
    { value: "week", label: m.tool_time_week() },
  ]}
>
  <LazyView
    loader={loadTimePanel}
    loadingLabel={m.panel_loading()}
    failureLabel={m.panel_load_failed()}
    retryLabel={m.panel_retry()}
    >{#snippet children(Panel)}<Panel
        profile={props.profile}
        span={props.state.filter === "week" ? "week" : "day"}
        privateWindow={tabs.profile()?.kind === "incognito"}
        onopen={expand}
      />{/snippet}</LazyView
  >
</ToolFrame>
