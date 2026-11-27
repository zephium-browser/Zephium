<script lang="ts">
  import * as m from "$shared/i18n/messages";
  import { tabs } from "$domain/tabs";
  import { favicons } from "$domain/favicons";
  import { ZoomLevelsSession } from "$domain/zoom";
  import { commands } from "$shared/ipc/bindings";
  import Button from "$shared/ui/Button";
  import FavIcon from "$shared/ui/FavIcon";
  import IconButton from "$shared/ui/IconButton";
  import SettingsRow from "$shared/ui/SettingsRow";
  import Cancel01Icon from "@hugeicons/core-free-icons/Cancel01Icon";
  import Globe02Icon from "@hugeicons/core-free-icons/Globe02Icon";

  const profile = $derived(tabs.profile()?.id ?? null);
  let session = $state.raw<ZoomLevelsSession | null>(null);
  $effect(() => {
    if (!profile) return;
    const owner = new ZoomLevelsSession(profile);
    session = owner;
    void owner.load().then(() => {
      const origins = owner.levels.map((level) => `https://${level.site}`);
      if (origins.length) void commands.faviconProbe(profile, origins).catch(() => {});
    });
    return () => owner.dispose();
  });
</script>

<SettingsRow
  settingId="appearance.zoom-levels"
  title={m.zoom_levels_title()}
  description={m.zoom_levels_help()}
>
  <Button
    size="compact"
    disabled={!session?.levels.length || session.busy}
    onclick={() => void session?.forgetAll()}>{m.zoom_levels_forget_all()}</Button
  >
</SettingsRow>
{#if session?.unavailable}
  <p class="note" role="alert">{m.zoom_levels_unavailable()}</p>
{:else if session?.loaded && session.levels.length === 0}
  <p class="note">{m.zoom_levels_empty()}</p>
{/if}
{#each session?.levels ?? [] as level (level.site)}
  {@const mark = favicons.forPage(`https://${level.site}`)}
  <div class="level">
    <span class="mark"
      ><FavIcon
        image={mark?.image ?? null}
        tone={mark?.tone}
        size={20}
        fallback={Globe02Icon}
      /></span
    >
    <span class="site">{level.site}</span>
    <span class="percent">{m.zoom_percent({ percent: level.percent })}</span>
    <IconButton
      icon={Cancel01Icon}
      size={14}
      buttonSize={26}
      label={m.zoom_levels_forget({ site: level.site })}
      disabled={session?.busy}
      onclick={() => void session?.forget(level.site)}
    />
  </div>
{/each}

<style>
  .level {
    position: relative;
    display: flex;
    align-items: center;
    gap: 12px;
    box-sizing: border-box;
    min-block-size: var(--row-page);
    padding: 10px 12px 10px 18px;
  }

  .level::before {
    content: "";
    position: absolute;
    inset-inline: 50px 0;
    inset-block-start: 0;
    block-size: 1px;
    background: var(--color-border);
  }

  .mark {
    display: grid;
    flex: none;
    place-items: center;
    inline-size: 20px;
    block-size: 20px;
  }

  .site {
    flex: 1;
    min-inline-size: 0;
    overflow: hidden;
    color: var(--color-text);
    font-size: var(--text-page-title);
    font-weight: 500;
    line-height: 19px;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .percent {
    flex: none;
    color: var(--color-muted);
    font-size: var(--text-label);
    font-variant-numeric: tabular-nums;
  }

  .note {
    position: relative;
    margin: 0;
    padding: 12px 18px;
    color: var(--color-muted);
    font-size: var(--text-label);
    line-height: 1.5;
  }

  .note::before {
    content: "";
    position: absolute;
    inset-inline: 16px 0;
    inset-block-start: 0;
    block-size: 1px;
    background: var(--color-border);
  }
</style>
