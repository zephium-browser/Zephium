<script lang="ts">
  import * as m from "$shared/i18n/messages";
  import type { HomeNode } from "../../lib/layout";
  import { dayName } from "../../lib/time";

  let { data }: { data: { item: Extract<HomeNode, { kind: "day" }> } } = $props();
  let item = $derived(data.item);
  let name = $derived(dayName(item.day.key, item.today) ?? m.home_yesterday());
</script>

<div
  class="day"
  style:inline-size={`${item.box.width}px`}
  style:block-size={`${item.box.height}px`}
>
  <h2>
    <span class="name">{name}</span>
    <span class="count"
      >{item.day.entries.length === 1
        ? m.home_day_count_one()
        : m.home_day_count({ count: item.day.entries.length })}</span
    >
  </h2>
</div>

<style>
  .day {
    display: grid;
    align-content: start;
  }

  h2 {
    display: inline-flex;
    align-items: baseline;
    justify-self: start;
    gap: 8px;
    margin: 0;
    padding: 7px 14px;
    border-radius: var(--radius-capsule);
    background: var(--color-surface);
    font-size: var(--text-label);
    font-weight: 600;
    line-height: 18px;
    white-space: nowrap;
  }

  .count {
    color: var(--color-faint);
    font-weight: 400;
  }
</style>
