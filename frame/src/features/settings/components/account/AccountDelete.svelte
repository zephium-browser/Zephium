<script lang="ts">
  import { Dialog } from "bits-ui";
  import { accountSession as session } from "$domain/account";
  import * as m from "$shared/i18n/messages";
  import Button from "$shared/ui/Button";
  import Field from "$shared/ui/Field";
  import SettingsRow from "$shared/ui/SettingsRow";
  import { codeDigits, explainAccountFault } from "../../lib/account";

  let open = $state(false);
  const sent = $derived(session.awaitingDeletionCode());
  let code = $state("");
  let opener: HTMLElement | null = null;
  const busy = $derived(session.pending());
  const fault = $derived(session.fault());
  const faultText = $derived(
    open && fault && (fault.action === "deletion_code" || fault.action === "delete")
      ? explainAccountFault(fault.fault, fault.retryAfter)
      : null,
  );

  async function submit() {
    if (!sent) {
      await session.sendDeletionCode();
      return;
    }
    const digits = codeDigits(code);
    if (digits.length !== 6) return;
    if (await session.deleteAccount(digits)) open = false;
  }
</script>

<SettingsRow title={m.account_delete()} description={m.account_delete_desc()}>
  <Button
    variant="ghost"
    size="compact"
    class="destructive"
    disabled={!!busy}
    onclick={() => {
      opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
      session.clearFault();
      code = "";
      open = true;
    }}>{m.account_delete_open()}</Button
  >
</SettingsRow>

<Dialog.Root bind:open>
  <Dialog.Portal>
    <Dialog.Overlay class="settings-dialog-overlay" />
    <Dialog.Content
      class="settings-dialog"
      onCloseAutoFocus={(event) => {
        if (opener?.isConnected) {
          event.preventDefault();
          opener.focus();
        }
      }}
    >
      <Dialog.Title class="settings-dialog-title">{m.account_delete_title()}</Dialog.Title>
      <Dialog.Description class="settings-dialog-description"
        >{m.account_delete_body()}</Dialog.Description
      >
      <form
        onsubmit={(event) => {
          event.preventDefault();
          void submit();
        }}
      >
        <div class="settings-dialog-body">
          {#if sent}<Field
              label={m.account_delete_code()}
              inputmode="numeric"
              autocomplete="one-time-code"
              placeholder="000000"
              maxlength={11}
              bind:value={code}
            />{/if}
          {#if faultText}<p class="error" role="alert">{faultText}</p>{/if}
        </div>
        <footer>
          <Button onclick={() => (open = false)}>{m.action_cancel()}</Button>
          {#if sent}<Button
              disabled={!!busy}
              onclick={async () => {
                if (await session.sendDeletionCode()) code = "";
              }}>{m.account_code_resend()}</Button
            >{/if}
          {#if !sent}<Button type="submit" pending={busy === "deletion_code"}
              >{m.account_delete_send()}</Button
            >{:else}<Button
              type="submit"
              variant="danger"
              pending={busy === "delete"}
              disabled={codeDigits(code).length !== 6}>{m.account_delete_confirm()}</Button
            >{/if}
        </footer>
      </form>
    </Dialog.Content>
  </Dialog.Portal>
</Dialog.Root>

<style>
  /* Quiet until reached for: the way out reads as a choice, never a call to action. */
  :global(
    .ui-settings-control
      > .ui-button.destructive[data-variant="ghost"]:is(:hover, :focus-visible):not(:disabled)
  ) {
    background: color-mix(in srgb, var(--color-danger) 14%, transparent);
    color: var(--color-danger);
  }

  .error {
    margin: 0;
    color: var(--color-danger);
    font-size: var(--text-label);
    line-height: 1.5;
  }
</style>
