<script lang="ts">
  import { accountSession as session } from "$domain/account";
  import * as m from "$shared/i18n/messages";
  import Button from "$shared/ui/Button";
  import Field from "$shared/ui/Field";
  import SettingsRow from "$shared/ui/SettingsRow";
  import { explainAccountFault } from "../../lib/account";

  const account = $derived(session.account());
  const busy = $derived(session.pending());
  const fault = $derived(session.fault());
  let editing = $state(false);
  let address = $state("");

  const host = $derived.by(() => {
    try {
      return new URL(account?.server ?? "").host;
    } catch {
      return account?.server ?? "";
    }
  });
  const faultText = $derived(
    fault?.action === "server" ? explainAccountFault(fault.fault, fault.retryAfter) : null,
  );

  async function save(server: string | null) {
    if (await session.setServer(server)) editing = false;
  }
</script>

<details class="advanced" open={account?.custom_server || undefined}>
  <summary><span>{m.account_server_group()}</span></summary>
  <div class="card">
    <SettingsRow
      title={m.account_server_title()}
      description={account?.custom_server ? host : m.account_server_default({ server: host })}
    >
      {#if account?.custom_server}<Button
          size="compact"
          variant="ghost"
          pending={busy === "server"}
          disabled={!!busy}
          onclick={() => void save(null)}>{m.account_server_reset()}</Button
        >{/if}
      {#if !editing}<Button
          size="compact"
          disabled={!!busy}
          onclick={() => {
            address = account?.custom_server ? (account.server ?? "") : "";
            editing = true;
          }}>{m.account_server_change()}</Button
        >{/if}
    </SettingsRow>
    {#if editing}<form
        class="editor"
        onsubmit={(event) => {
          event.preventDefault();
          void save(address.trim());
        }}
      >
        <Field
          label={m.account_server_field()}
          labelHidden
          type="url"
          placeholder="https://accounts.example.com"
          autocomplete="off"
          spellcheck="false"
          bind:value={address}
          onkeydown={(event) => {
            if (event.key === "Escape") editing = false;
          }}
        />
        <Button size="compact" variant="ghost" onclick={() => (editing = false)}
          >{m.action_cancel()}</Button
        >
        <Button
          size="compact"
          variant="primary"
          type="submit"
          pending={busy === "server"}
          disabled={!address.trim()}>{m.account_server_save()}</Button
        >
      </form>{/if}
    {#if faultText}<p class="error" role="alert">{faultText}</p>{/if}
  </div>
  <p class="hint">{m.account_server_note()}</p>
</details>

<style>
  .advanced {
    margin-block: 8px 34px;
  }

  /* At the foot of the sign-in scene, in the sheet's column. */
  :global(.account-gate) .advanced {
    inline-size: min(100%, 400px);
    margin-block: 0;
    text-align: center;
  }

  summary {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 4px 16px;
    color: var(--color-muted);
    font-size: var(--text-label);
    font-weight: 500;
    list-style: none;
    cursor: default;
  }

  summary::-webkit-details-marker {
    display: none;
  }

  summary::before {
    inline-size: 0;
    block-size: 0;
    border-block: 4px solid transparent;
    border-inline-start: 5px solid currentcolor;
    content: "";
    transition: rotate var(--motion-fast) var(--ease-out);
  }

  .advanced[open] summary::before {
    rotate: 90deg;
  }

  summary:hover {
    color: var(--color-text);
  }

  .card {
    overflow: hidden;
    margin-block-start: 10px;
    border-radius: var(--radius-card);
    background: var(--color-surface);
    box-shadow: var(--shadow-card);
  }

  :global(.account-gate) .card {
    text-align: start;
  }

  .hint {
    margin: 8px 16px 0;
    color: var(--color-muted);
    font-size: var(--text-label);
    line-height: 1.5;
  }

  @media (prefers-reduced-motion: reduce) {
    summary::before {
      transition: none;
    }
  }

  .editor {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 0 18px 14px;
  }

  .editor :global(.ui-field) {
    flex: 1;
  }

  .error {
    margin: 0;
    padding: 0 18px 14px;
    color: var(--color-danger);
    font-size: var(--text-label);
  }
</style>
