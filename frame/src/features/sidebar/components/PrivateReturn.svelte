<script lang="ts">
  import * as m from "$shared/i18n/messages";
  import { onMount } from "svelte";
  import { IncognitoIcon } from "@hugeicons/core-free-icons";
  import { commands } from "$shared/ipc/bindings";
  import { profiles } from "$domain/profiles";
  import Icon from "$shared/ui/Icon";
  import IconButton from "$shared/ui/IconButton";

  let { compact = false }: { compact?: boolean } = $props();
  onMount(() => {
    void profiles.retain();
    return () => profiles.release();
  });
  const aside = $derived(profiles.privateAside());
  const count = $derived(profiles.privateTabs() ?? 0);
  const label = $derived(`${m.private_return()}. ${m.private_tabs_open({ count })}`);
  const show = () => void commands.runCommand("window.showPrivate").catch(() => {});
</script>

<!--
  While the private window is set aside the regular column still shows it:
  its tabs are open and one press away, so it can never be lost for want of
  a button.
-->
{#if aside}
  {#if compact}
    <IconButton icon={IncognitoIcon} {label} size={16} onclick={show} />
  {:else}
    <button type="button" class="private-return" title={label} aria-label={label} onclick={show}>
      <Icon icon={IncognitoIcon} size={15} />
      <span class="text">{m.private_return()}</span>
      <span class="count" aria-hidden="true">{count}</span>
    </button>
  {/if}
{/if}

<style>
  .private-return {
    display: flex;
    flex: none;
    align-items: center;
    gap: 8px;
    box-sizing: border-box;
    inline-size: calc(100% - 2 * var(--sidebar-inset));
    block-size: 32px;
    margin: 6px var(--sidebar-inset) 0;
    padding-inline: 10px 8px;
    border: 0;
    border-radius: var(--radius-row);
    background: var(--row-active);
    box-shadow: var(--row-rim);
    color: var(--color-text);
    font: inherit;
    font-size: var(--text-label);
    font-weight: 500;
    text-align: start;
    cursor: default;
    transition:
      background-color var(--motion-fast) var(--ease-out),
      scale var(--motion-slow) var(--ease-spring);
  }

  .private-return:hover {
    background: var(--row-pressed);
  }

  .private-return:active {
    scale: 0.98;
    transition-duration: var(--motion-instant);
  }

  .private-return:focus-visible {
    outline: 2px solid var(--color-ring);
    outline-offset: 2px;
  }

  .private-return :global(svg) {
    flex: none;
    color: var(--color-muted);
  }

  .text {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .count {
    flex: none;
    min-inline-size: 20px;
    padding-inline: 6px;
    border-radius: var(--radius-capsule);
    background: var(--color-fill);
    color: var(--color-muted);
    font-size: var(--text-caption);
    font-variant-numeric: tabular-nums;
    line-height: 20px;
    text-align: center;
  }
</style>
