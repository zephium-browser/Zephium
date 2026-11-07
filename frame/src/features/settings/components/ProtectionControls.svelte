<script lang="ts">
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
      ? "Protection starting… You can keep browsing."
      : status.protection === "active"
        ? "Protection is active."
        : status.protection === "disabled"
          ? "Protection is off for this profile."
          : "Protection needs attention. The last working rules are retained when available.",
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
      if (result.state === "pending") message = "The change is still being applied.";
      else if (
        result.state !== "processed" ||
        !["applied", "no_op"].includes(result.disposition.outcome)
      )
        message = "Could not confirm this change. Check the status or retry.";
    } finally {
      busy = false;
    }
  }
</script>

<SettingsGroup
  title="Ad & tracker protection"
  description="These controls change live protection for this profile. Site pauses and saved element hides are available from the page’s quick menu."
>
  <SettingsRow title="Block ads and trackers" description={standing} settingId="privacy.blocking">
    <Switch
      label="Block ads and trackers"
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
    title="Filter lists"
    description="EasyList, EasyPrivacy, and uBlock Origin's filters and quick fixes from their official HTTPS publishers. Updates are validated before activation, with a built-in offline fallback."
  >
    <Button
      disabled={busy || !status.can_refresh_sources}
      onclick={() => void change(blocker.refreshSources)}
      >{status.source_phase === "refreshing" ? "Updating…" : "Check for updates"}</Button
    >
  </SettingsRow>
  {#if status.retryable && status.desired_generation}
    <SettingsRow
      title="Retry protection"
      description="Retry preparing the current rules without interrupting browsing."
      ><Button
        disabled={busy}
        onclick={() => {
          if (status.desired_generation)
            void change(() => blocker.retry(status.desired_generation!));
        }}>Retry</Button
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
