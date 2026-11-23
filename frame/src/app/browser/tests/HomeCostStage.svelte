<script lang="ts">
  import { createRawSnippet } from "svelte";
  import LazyView from "$shared/ui/LazyView";
  import { loadHome } from "$features/home";
  import { loadNewTab } from "$features/newtab";

  let { home = false }: { home?: boolean } = $props();
  const search = createRawSnippet(() => ({
    render: () => '<div class="dock"><input aria-label="Search" /></div>',
  }));
</script>

{#snippet canvas()}<LazyView loader={loadHome} loadingLabel="" failureLabel="" retryLabel=""
    >{#snippet children(Home)}<Home />{/snippet}</LazyView
  >{/snippet}

{#await loadNewTab() then { default: NewTab }}<NewTab
    {search}
    canvas={home ? canvas : undefined}
    welcome={!home}
    oncustomize={() => {}}
  />{/await}
