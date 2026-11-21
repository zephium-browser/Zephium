<script lang="ts">
  import { onMount } from "svelte";
  import { accountSession as session, planName } from "$domain/account";
  import { preferences } from "$domain/preferences";
  import * as m from "$shared/i18n/messages";
  import Button from "$shared/ui/Button";
  import ZephiumTile from "$shared/ui/ZephiumTile";
  import SettingsGroup from "$shared/ui/SettingsGroup";
  import * as settings from "../lib/settings-state.svelte";

  onMount(() => {
    void session.retain();
    return () => session.release();
  });

  const account = $derived(session.account());
  /** Shown only where hosted models exist: a self-hosted server may offer none. */
  const offered = $derived(
    !!account?.available &&
      (account.signed_in || (account.hosted_models === true && preferences.showSignInPrompts())),
  );
  const detail = $derived.by(() => {
    if (!account?.signed_in) return m.ai_cloud_signed_out();
    const plan = planName(account.plan);
    const used = account.usage?.used_percent;
    return [
      plan ? m.ai_cloud_signed_in_plan({ plan }) : m.ai_cloud_signed_in(),
      used != null ? m.ai_cloud_used({ percent: used }) : null,
    ]
      .filter(Boolean)
      .join(" · ");
  });
</script>

{#if offered}<SettingsGroup title={m.ai_cloud_group()}>
    <div class="zephium-row">
      <ZephiumTile size={32} />
      <div class="copy">
        <h3>{m.ai_cloud_title()}</h3>
        <p>{detail}</p>
      </div>
      <Button
        size="compact"
        variant={account?.signed_in ? "secondary" : "primary"}
        onclick={() => settings.select("account")}
        >{account?.signed_in ? m.ai_cloud_account() : m.ai_cloud_sign_in()}</Button
      >
    </div>
  </SettingsGroup>{/if}

<style>
  /* Metrics match AiProviderRow so the group reads as one list. */
  .zephium-row {
    display: flex;
    align-items: center;
    gap: 12px;
    box-sizing: border-box;
    min-height: var(--row-page);
    padding: 12px 18px 12px 14px;
  }

  .copy {
    flex: 1;
    min-width: 0;
  }

  h3 {
    margin: 0;
    color: var(--color-text);
    font-size: var(--text-page-title);
    font-weight: 500;
    line-height: 19px;
    letter-spacing: -0.008em;
  }

  p {
    margin: 2px 0 0;
    overflow: hidden;
    color: var(--color-muted);
    font-size: var(--text-label);
    line-height: 1.5;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
