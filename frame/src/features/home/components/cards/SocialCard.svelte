<script lang="ts">
  import { getContext } from "svelte";
  import { focusStop } from "$shared/canvas";
  import Icon from "$shared/ui/Icon";
  import * as m from "$shared/i18n/messages";
  import { homeTop, type HomeTop } from "../../lib/context";
  import { SIZE, type HomeNode } from "../../lib/layout";
  import { NETWORK_MARK, NETWORK_NAME } from "../../lib/marks";
  import Card from "./Card.svelte";

  let { id, data }: { id: string; data: { item: Extract<HomeNode, { kind: "social" }> } } =
    $props();
  const top = getContext<HomeTop>(homeTop);
  const at = focusStop();
  let item = $derived(data.item);
</script>

<Card width={item.box.width} height={item.box.height} label={m.home_elsewhere()}>
  <p class="label" style:block-size={`${SIZE.label}px`}>{m.home_elsewhere()}</p>
  <div class="grid">
    {#each item.place.social as social (social)}
      <button
        type="button"
        style:block-size={`${SIZE.section}px`}
        {@attach at(`${id}/${social}`)}
        onclick={() =>
          top.intent({
            kind: "social",
            search: item.search.id,
            place: item.place.id,
            network: social,
          })}
      >
        <Icon icon={NETWORK_MARK[social]} size={14} />
        <span>{NETWORK_NAME[social]}</span>
      </button>
    {/each}
  </div>
</Card>

<style>
  .label {
    box-sizing: border-box;
    flex: none;
    margin: 0;
    padding: 8px 10px 0;
    color: var(--color-faint);
    font-size: var(--text-caption);
    font-weight: 600;
  }

  .grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
  }

  button {
    display: flex;
    align-items: center;
    gap: 7px;
    min-inline-size: 0;
    padding-inline: 10px;
    border: 0;
    border-radius: var(--radius-control-compact);
    background: none;
    color: var(--color-label-secondary);
    font: inherit;
    font-size: var(--text-label);
    cursor: default;
  }

  button:hover {
    background: var(--row-hover);
    color: var(--color-text);
  }

  button :global(svg) {
    flex: none;
    color: var(--color-muted);
  }

  span {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
