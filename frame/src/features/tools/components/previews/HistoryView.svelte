<script lang="ts">
  import type { ToolHostProps } from "$session/tool-drafts.svelte";
  import { surface as browser } from "$domain/surface";
  import { loadHistoryPanel } from "$features/history";
  import LazyView from "$shared/ui/LazyView";
  import ToolFrame from "../ToolFrame.svelte";
  import * as m from "$shared/i18n/messages";
  let props: ToolHostProps = $props();

  // The page is the same history in more depth, so the panel does not stay
  // open behind it.
  function expand() {
    void browser.open("history");
    props.onclose();
  }
</script>

<ToolFrame
  {...props}
  caption={false}
  onexpand={expand}
  searchLabel={m.history_search()}
  scrolls={false}
>
  <LazyView
    loader={loadHistoryPanel}
    loadingLabel={m.panel_loading()}
    failureLabel={m.panel_load_failed()}
    retryLabel={m.panel_retry()}
    >{#snippet children(Panel)}<Panel
        profile={props.profile}
        query={props.state.query}
      />{/snippet}</LazyView
  >
</ToolFrame>
