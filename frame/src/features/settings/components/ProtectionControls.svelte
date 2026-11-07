<script lang="ts">
  import * as m from "$shared/i18n/messages";
  import { onMount } from "svelte";
  import { blocker } from "$domain/blocker";
  import SettingsGroup from "$shared/ui/SettingsGroup";
  import SettingsRow from "$shared/ui/SettingsRow";
  import Switch from "$shared/ui/Switch";
  import Button from "$shared/ui/Button";

  let status = $derived(blocker.status());
  let busy = $state(false);
  let message = $state("");
  let standing = $derived(
    status.protection === "pending"
      ? m.settings_protection_starting()
      : status.protection === "active"
        ? m.settings_protection_active()
        : status.protection === "disabled"
          ? m.settings_protection_off()
          : m.settings_protection_attention(),
  );
  onMount(() => {
    void blocker.refresh();
  });
  async function change(action: () => Promise<blocker.BlockerMutationResult>) {
    if (busy) return;
    busy = true;
    message = "";
    try {
      const result = await action();
      if (result.state === "pending") message = m.settings_protection_change_pending();
      else if (
        result.state !== "processed" ||
        !["applied", "no_op"].includes(result.disposition.outcome)
      )
        message = m.settings_protection_change_failed();
    } finally {
      busy = false;
    }
  }
</script>

<SettingsGroup title={m.settings_protection()} description={m.settings_protection_help()}>
  <SettingsRow
    title={m.settings_protection_enabled()}
    description={standing}
    settingId="privacy.blocking"
  >
    <Switch
      label={m.settings_protection_enabled()}
      labelHidden
      checked={status.desired_enabled === true}
      disabled={busy ||
        status.preference !== "authoritative" ||
        status.protection === "pending" ||
        (status.desired_enabled !== true && !status.can_enable)}
      onchange={(enabled) => void change(() => blocker.setEnabled(enabled))}
    />
  </SettingsRow>
  <SettingsRow
    title={m.settings_protection_lists()}
    description={m.settings_protection_lists_help()}
  >
    <Button
      disabled={busy || !status.can_refresh_sources}
      onclick={() => void change(blocker.refreshSources)}
      >{status.source_phase === "refreshing"
        ? m.settings_protection_updating()
        : m.settings_protection_check_updates()}</Button
    >
  </SettingsRow>
  {#if status.retryable && status.desired_generation}
    <SettingsRow
      title={m.settings_protection_retry()}
      description={m.settings_protection_retry_help()}
      ><Button
        disabled={busy}
        onclick={() => {
          if (status.desired_generation)
            void change(() => blocker.retry(status.desired_generation!));
        }}>{m.settings_protection_retry_action()}</Button
      ></SettingsRow
    >
  {/if}
  {#if message}<p class="feedback" role="status">{message}</p>{/if}
</SettingsGroup>

<style>
  .feedback {
    margin: 12px 18px;
    color: var(--color-muted);
    font-size: var(--text-body);
  }
</style>
