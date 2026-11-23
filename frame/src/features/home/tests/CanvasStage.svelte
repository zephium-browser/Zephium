<script lang="ts">
  import { untrack } from "svelte";
  import type { HomeModel, Place } from "$domain/home";
  import HomeCanvas from "../components/HomeCanvas.svelte";
  import { Front } from "../lib/front.svelte";
  import type { HomeIntent } from "../lib/intent";

  let {
    home,
    onintent = () => {},
    onready,
    marked = null,
    show = null,
    mark,
  }: {
    home: HomeModel;
    onintent?: (intent: HomeIntent) => void;
    onready?: (enter: () => void) => void;
    marked?: { search: string; entry: string } | null;
    /** A search to show, as native asks after a page of it closes. */
    show?: string | null;
    mark?: (place: Place) => ImageData | null;
  } = $props();

  // The canvas as the stage holds it: which searches are open is the stage's to say.
  const front = new Front();
  $effect.pre(() => {
    const entries = [home.today, ...home.earlier].flatMap((day) => day.entries);
    const newest = entries.find((entry) => entry.kind !== "address")?.id ?? null;
    const held = new Set(entries.map((entry) => entry.id));
    untrack(() => front.follow(newest, held));
  });
  $effect.pre(() => {
    const search = show;
    if (search) untrack(() => front.show(search));
  });
</script>

<div class="stage">
  <HomeCanvas
    {home}
    open={front.open}
    lead={front.lead}
    centre={front.centre}
    {marked}
    onopen={(search) => front.choose(search)}
    {onintent}
    {onready}
    {mark}
  />
</div>

<style>
  .stage {
    position: fixed;
    inset: 0;
    background: var(--color-canvas);
  }
</style>
