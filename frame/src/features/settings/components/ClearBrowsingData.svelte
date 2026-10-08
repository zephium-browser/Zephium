<script lang="ts">
  import { Dialog } from "bits-ui";
  import { commands, type HistoryRange } from "$shared/ipc/bindings";
  import { tabs } from "$domain/tabs";
  import * as m from "$shared/i18n/messages";
  import Button from "$shared/ui/Button";
  import Checkbox from "$shared/ui/Checkbox";
  import Select from "$shared/ui/Select";
  import SettingsRow from "$shared/ui/SettingsRow";
  import { fields } from "../lib/catalog";

  const field = fields["privacy.clear"];
  let open = $state(false);
  let range = $state<HistoryRange>("hour");
  let history = $state(true);
  let pending = $state(false);
  let failed = $state(false);
  let cleared = $state(false);
  let opener: HTMLElement | null = null;

  async function clear() {
    const profile = tabs.profile()?.id;
    if (!profile || !history || pending) return;
    pending = true;
    failed = false;
    try {
      const result = await commands.historyCall(profile, { kind: "clear", range });
      if (result.kind !== "removed") {
        failed = true;
        return;
      }
      cleared = true;
      open = false;
    } catch {
      failed = true;
    } finally {
      pending = false;
    }
  }
</script>

<div data-setting="privacy.clear">
  <SettingsRow title={field.label()} description={field.description()}>
    <Button
      onclick={() => {
        opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
        cleared = false;
        failed = false;
        open = true;
      }}>{m.privacy_clear_open()}</Button
    >
  </SettingsRow>
  {#if cleared}<p class="result" role="status">{m.privacy_clear_done()}</p>{/if}
</div>

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
      <Dialog.Title class="settings-dialog-title">{field.label()}</Dialog.Title>
      <Dialog.Description class="settings-dialog-description"
        >{m.privacy_clear_description()}</Dialog.Description
      >
      <form
        onsubmit={(event) => {
          event.preventDefault();
          void clear();
        }}
      >
        <div class="settings-dialog-body">
          <Select
            label={m.preview_time_range()}
            value={range}
            options={[
              { value: "hour", label: m.history_range_hour() },
              { value: "day", label: m.history_range_day() },
              { value: "week", label: m.history_range_week() },
              { value: "everything", label: m.history_range_all() },
            ]}
            onchange={(value) => (range = value as HistoryRange)}
          />
          <Checkbox
            label={m.preview_history()}
            checked={history}
            onchange={(value) => (history = value)}
          />
          {#if failed}<p class="result error" role="alert">{m.privacy_clear_failed()}</p>{/if}
        </div>
        <footer>
          <Button onclick={() => (open = false)}>{m.action_cancel()}</Button>
          <Button type="submit" variant="primary" disabled={!history} {pending}
            >{m.privacy_clear_apply()}</Button
          >
        </footer>
      </form>
    </Dialog.Content>
  </Dialog.Portal>
</Dialog.Root>

<style>
  .result {
    margin: 12px 18px;
    color: var(--color-muted);
    font-size: var(--text-body);
  }

  .error {
    margin-inline: 0;
    color: var(--color-danger);
  }
</style>
