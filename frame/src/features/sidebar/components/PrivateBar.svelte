<script lang="ts">
  import * as m from "$shared/i18n/messages";
  import { onMount } from "svelte";
  import { ArrowLeft02Icon, Cancel01Icon, IncognitoIcon } from "@hugeicons/core-free-icons";
  import { commands } from "$shared/ipc/bindings";
  import { profiles } from "$domain/profiles";
  import Button from "$shared/ui/Button";
  import Icon from "$shared/ui/Icon";
  import IconButton from "$shared/ui/IconButton";

  let { compact = false }: { compact?: boolean } = $props();
  onMount(() => {
    void profiles.retain();
    return () => profiles.release();
  });
  // The profile the window goes back to, whose tabs were left as they were.
  const name = $derived(profiles.current()?.name ?? "");
  const run = (id: string) => void commands.runCommand(id).catch(() => {});
  const back = () => run("window.showRegular");
  const close = () => run("window.closePrivate");
</script>

<!--
  The foot of the column while it shows private tabs, where tools and kept
  sites stand otherwise. It names the scope and offers two ways out that do
  not look alike: back sets the private tabs aside, still open and one press
  away; close erases them.
-->
{#if compact}
  <div class="private-rail" role="group" aria-label={m.private_title()}>
    <Icon icon={IncognitoIcon} size={16} />
    {#if name}<IconButton
        icon={ArrowLeft02Icon}
        label={m.private_back({ name })}
        size={16}
        onclick={back}
      />{/if}
    <IconButton icon={Cancel01Icon} label={m.private_close()} size={16} onclick={close} />
  </div>
{:else}
  <div class="private-bar" role="group" aria-label={m.private_title()}>
    <div class="head">
      <Icon icon={IncognitoIcon} size={16} />
      <div class="copy">
        <span class="name">{m.private_title()}</span>
        <span class="explain">{m.private_explain()}</span>
      </div>
    </div>
    {#if name}<Button class="action" title={m.private_back_hint({ name })} onclick={back}>
        <Icon icon={ArrowLeft02Icon} size={15} />
        <span class="label">{m.private_back({ name })}</span>
      </Button>{/if}
    <Button class="action" variant="danger" title={m.private_close_hint()} onclick={close}>
      <Icon icon={Cancel01Icon} size={15} />
      <span class="label">{m.private_close()}</span>
    </Button>
  </div>
{/if}

<style>
  .private-bar {
    display: flex;
    flex: none;
    flex-direction: column;
    box-sizing: border-box;
    inline-size: 100%;
    gap: 6px;
    min-width: 0;
    padding: 8px;
    border-radius: var(--radius-card);
    background: var(--color-fill);
    color: var(--color-text);
  }

  .head {
    display: flex;
    align-items: flex-start;
    gap: 8px;
    padding: 2px 2px 4px;
  }

  .head :global(svg) {
    flex: none;
    margin-block-start: 1px;
  }

  .copy {
    display: grid;
    gap: 1px;
    min-width: 0;
  }

  .name {
    font-size: 13px;
    font-weight: 500;
    line-height: 16px;
  }

  .explain {
    color: var(--color-muted);
    font-size: var(--text-caption);
    line-height: 15px;
  }

  .private-bar :global(.action) {
    inline-size: 100%;
    min-width: 0;
  }

  .label {
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .private-rail {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 4px;
    color: var(--color-muted);
  }
</style>
