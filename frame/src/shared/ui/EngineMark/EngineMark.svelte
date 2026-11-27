<script lang="ts" module>
  import google from "./marks/google.png?no-inline";
  import duckduckgo from "./marks/duckduckgo.png?no-inline";
  import bing from "./marks/bing.png?no-inline";
  import brave from "./marks/brave.png?no-inline";
  import kagi from "./marks/kagi.png?no-inline";
  import perplexity from "./marks/perplexity.png?no-inline";
  import chatgpt from "./marks/chatgpt.png?no-inline";
  import claude from "./marks/claude.png?no-inline";
  import wikipedia from "./marks/wikipedia.png?no-inline";
  import youtube from "./marks/youtube.png?no-inline";
  import googleImages from "./marks/google-images.png?no-inline";
  import googleMaps from "./marks/google-maps.png?no-inline";
  import github from "./marks/github.png?no-inline";
  import reddit from "./marks/reddit.png?no-inline";
  import x from "./marks/x.png?no-inline";
  import amazon from "./marks/amazon.png?no-inline";
  import mdn from "./marks/mdn.png?no-inline";
  import stackoverflow from "./marks/stackoverflow.png?no-inline";
  import hackernews from "./marks/hackernews.png?no-inline";
  import translate from "./marks/translate.png?no-inline";
  import ebay from "./marks/ebay.png?no-inline";
  import wolframalpha from "./marks/wolframalpha.png?no-inline";
  import archwiki from "./marks/archwiki.png?no-inline";
  import docsRs from "./marks/docs-rs.png?no-inline";
  import twitch from "./marks/twitch.png?no-inline";
  import spotify from "./marks/spotify.png?no-inline";
  import startpage from "./marks/startpage.png?no-inline";
  import internetArchive from "./marks/internet-archive.png?no-inline";

  /** The marks of the default engines and shortcuts, shipped with the app, so
   *  each shows its own before any of its sites has been visited and without
   *  ever reaching a site for it. Keyed by the shortcut's id. */
  const MARKS: Record<string, string> = {
    google,
    duckduckgo,
    bing,
    brave,
    kagi,
    perplexity,
    chatgpt,
    claude,
    wikipedia,
    youtube,
    "google-images": googleImages,
    "google-maps": googleMaps,
    github,
    reddit,
    x,
    amazon,
    mdn,
    stackoverflow,
    hackernews,
    translate,
    ebay,
    wolframalpha,
    archwiki,
    "docs-rs": docsRs,
    twitch,
    spotify,
    startpage,
    "internet-archive": internetArchive,
  };

  /** The shipped mark of a default engine or shortcut, if it is one. */
  export const engineMarkUrl = (engine: string): string | undefined =>
    Object.hasOwn(MARKS, engine) ? MARKS[engine] : undefined;
</script>

<script lang="ts">
  let { engine, size = 16 }: { engine: string; size?: number } = $props();
  let src = $derived(engineMarkUrl(engine));
</script>

{#if src}<img class="engine-mark" {src} width={size} height={size} alt="" draggable="false" />{/if}

<style>
  .engine-mark {
    display: block;
    flex: none;
    border-radius: 3px;
  }
</style>
