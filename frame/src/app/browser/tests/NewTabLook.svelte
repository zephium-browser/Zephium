<script lang="ts">
  import "$styles/global.css";
  import { loadNewTab } from "$features/newtab";
  import { loadNewTabSearch } from "$features/search";
  import LazyView from "$shared/ui/LazyView";

  let {
    ontasks,
    onaccount,
  }: { ontasks?: () => void; onaccount?: (intent: "account" | "credits") => void } = $props();
  const rows = ["Terax", "Proton VPN", "Wikipedia", "Figma", "ChatGPT", "New Tab"];
</script>

<!-- The composition Shell draws: a sidebar beside the pane that lets the page
     paint its own ground, so the ground each takes can be seen together. The
     sidebar here is a stand-in with the real one's class and nothing else. -->
<div class="shell" style="display: flex; block-size: 100%">
  <aside class="browser-sidebar" style="position: relative; inline-size: 200px; padding: 44px 10px">
    {#each rows as row (row)}<p style="margin: 0; padding: 7px 10px; font-size: 13px">
        {row}
      </p>{/each}
  </aside>
  <div style="flex: 1; min-inline-size: 0; padding-inline-start: 8px">
    <div class="content-pane" data-ground="own" style="block-size: 100%">
      <LazyView loader={loadNewTab} loadingLabel="" failureLabel="failed" retryLabel="retry"
        >{#snippet children(NewTab)}<NewTab oncustomize={() => {}} {ontasks} {onaccount}
            >{#snippet search()}<LazyView
                loader={loadNewTabSearch}
                loadingLabel=""
                failureLabel="failed"
                retryLabel="retry"
                >{#snippet children(Search)}<Search
                    tabId="tab"
                    loadMenu={() => import("$shared/ui/Menu")}
                  />{/snippet}</LazyView
              >{/snippet}</NewTab
          >{/snippet}</LazyView
      >
    </div>
  </div>
</div>
