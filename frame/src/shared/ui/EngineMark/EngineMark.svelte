<script lang="ts" module>
  import google from "./marks/google.png?no-inline";
  import duckduckgo from "./marks/duckduckgo.png?no-inline";
  import bing from "./marks/bing.png?no-inline";
  import brave from "./marks/brave.png?no-inline";

  /** The default engines' own marks, shipped with the app, so a choice
   *  between them shows them before any site has been visited. */
  const MARKS: Record<string, string> = { google, duckduckgo, bing, brave };

  /** The shipped mark of a default engine, if it is one. */
  export const engineMarkUrl = (engine: string): string | undefined => MARKS[engine];
</script>

<script lang="ts">
  let { engine, size = 16 }: { engine: string; size?: number } = $props();
  let src = $derived(MARKS[engine]);
</script>

{#if src}<img class="engine-mark" {src} width={size} height={size} alt="" draggable="false" />{/if}

<style>
  .engine-mark {
    display: block;
    flex: none;
    border-radius: 3px;
  }
</style>
