<script lang="ts">
  import { onMount } from "svelte";
  import { accountSession as session } from "$domain/account";
  import { profiles } from "$domain/profiles";
  import * as m from "$shared/i18n/messages";
  import { IS_WINDOWS } from "$shared/platform";
  import Button from "$shared/ui/Button";
  import SettingsRow from "$shared/ui/SettingsRow";
  import AccountDelete from "../account/AccountDelete.svelte";
  import AccountDevices from "../account/AccountDevices.svelte";
  import AccountGate from "../account/AccountGate.svelte";
  import AccountIdentity from "../account/AccountIdentity.svelte";
  import AccountPlan from "../account/AccountPlan.svelte";
  import AccountServer from "../account/AccountServer.svelte";
  import AccountSignIn from "../account/AccountSignIn.svelte";
  import { explainAccountFault } from "../../lib/account";

  onMount(() => {
    void session.retain().then(() => session.refresh());
    void profiles.retain();
    return () => {
      session.release();
      profiles.release();
    };
  });

  const account = $derived(session.account());
  const busy = $derived(session.pending());
  const fault = $derived(session.fault());
  const ended = $derived(
    fault?.fault === "signed_out" ? explainAccountFault("signed_out", null) : null,
  );
  const signOutFault = $derived(
    fault?.action === "sign_out" ? explainAccountFault(fault.fault, fault.retryAfter) : null,
  );
  const moved = $derived(session.movedTo());
  /** What stands in the sign-in sheet before anyone has typed anything. */
  const notices = $derived([
    ...(account?.update_required
      ? [{ text: m.account_update_required(), tone: "warning" as const }]
      : []),
    ...(moved
      ? [{ text: m.account_email_moved({ email: moved }) }]
      : ended
        ? [{ text: ended }]
        : []),
  ]);
  let exported = $state<"saved" | string | null>(null);

  async function exportData() {
    exported = null;
    const result = await session.exportData();
    if (result === "saved") exported = "saved";
    else if (result !== "cancelled") exported = explainAccountFault(result, null);
  }
</script>

{#if session.unavailable()}<p class="note" role="status">{m.account_unavailable()}</p>
{:else if account}
  {#if !account.signed_in}
    <AccountGate>
      <AccountSignIn {notices} />
      {#snippet foot()}<AccountServer />{/snippet}
    </AccountGate>
  {:else}
    {#if account.update_required}<p class="note" data-tone="warning" role="status">
        {m.account_update_required()}
      </p>{/if}
    <AccountIdentity />
    {#if account.vault_fault}<div class="fault" role="alert">
        <p>{IS_WINDOWS ? m.account_vault_fault_windows() : m.account_vault_fault_mac()}</p>
        <Button
          size="compact"
          variant="ghost"
          pending={busy === "sign_out"}
          disabled={!!busy && busy !== "sign_out"}
          onclick={() => void session.signOut()}>{m.account_sign_in_again()}</Button
        >
      </div>{/if}
    {#if account.notice}<p class="note" data-tone={account.notice.warning ? "warning" : undefined}>
        {account.notice.text}
      </p>{/if}
    <AccountPlan />
    <AccountDevices />
    <section class="quiet" aria-labelledby="account-data">
      <h2 id="account-data">{m.account_data_group()}</h2>
      <SettingsRow title={m.account_export_title()} description={m.account_export_desc()}>
        {#if exported === "saved"}<span class="saved" role="status">{m.account_export_saved()}</span
          >{/if}
        <Button
          size="compact"
          variant="ghost"
          pending={busy === "export"}
          disabled={!!busy && busy !== "export"}
          onclick={() => void exportData()}>{m.account_export()}</Button
        >
      </SettingsRow>
      <SettingsRow title={m.account_sign_out_title()} description={m.account_sign_out_desc()}>
        <Button
          size="compact"
          variant="ghost"
          pending={busy === "sign_out"}
          disabled={!!busy && busy !== "sign_out"}
          onclick={() => void session.signOut()}>{m.account_sign_out()}</Button
        >
      </SettingsRow>
      <AccountDelete />
    </section>
    {#if signOutFault}<p class="note" data-tone="error" role="alert">{signOutFault}</p>{/if}
    {#if exported && exported !== "saved"}<p class="note" data-tone="error" role="alert">
        {exported}
      </p>{/if}
    <AccountServer />
  {/if}
{/if}

<style>
  /* The sign-in scene, or the person's own name, is the page's heading: the
     section's title stays only for assistive technology. */
  :global(.settings-view:has(:is(.account-gate, .account-identity)) > .settings-page-heading) {
    position: absolute;
    overflow: hidden;
    inline-size: 1px;
    block-size: 1px;
    clip-path: inset(50%);
    white-space: nowrap;
  }

  /* Data and the way out sit under everything else, without a card: they
     matter, but they are not what anyone opened Account to do. */
  .quiet {
    margin-block: 4px 22px;
  }

  .quiet h2 {
    margin: 0 0 2px;
    padding-inline: 18px;
    color: var(--color-faint);
    font-size: var(--text-caption);
    font-weight: 600;
    line-height: 20px;
    letter-spacing: 0.02em;
  }

  .quiet :global(.ui-settings-row) {
    min-block-size: 0;
    padding-block: 10px;
  }

  .quiet :global(.ui-settings-copy h3) {
    color: var(--color-label-secondary);
    font-size: var(--text-body);
  }

  .quiet :global(.ui-settings-copy p) {
    color: var(--color-faint);
  }

  .quiet :global(.ui-settings-control) {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-inline-end: -10px;
  }

  .note {
    margin: 0 18px 24px;
    color: var(--color-muted);
    font-size: var(--text-label);
    line-height: 1.5;
  }

  .fault {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
    margin: 0 18px 24px;
  }

  .fault p {
    margin: 0;
    color: var(--color-warning);
    font-size: var(--text-label);
    line-height: 1.5;
  }

  .note[data-tone="error"] {
    color: var(--color-danger);
  }

  .note[data-tone="warning"] {
    color: var(--color-warning);
  }

  .saved {
    color: var(--color-muted);
    font-size: var(--text-label);
  }
</style>
