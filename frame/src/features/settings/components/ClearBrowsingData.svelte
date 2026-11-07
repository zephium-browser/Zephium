<script lang="ts">
  import { Dialog } from "bits-ui";
  import type { BrowsingDataRange } from "$shared/ipc/bindings";
  import { tabs } from "$domain/tabs";
  import { clearBrowsingData, type ClearKinds } from "$domain/site-data";
  import * as m from "$shared/i18n/messages";
  import Button from "$shared/ui/Button";
  import Checkbox from "$shared/ui/Checkbox";
  import Select from "$shared/ui/Select";
  import SettingsRow from "$shared/ui/SettingsRow";
  import { fields } from "../lib/catalog";

  const field = fields["privacy.clear"];
  const ranges: ReadonlyArray<{ value: BrowsingDataRange; label: () => string }> = [
    { value: "hour", label: m.privacy_clear_range_hour },
    { value: "day", label: m.privacy_clear_range_day },
    { value: "week", label: m.privacy_clear_range_week },
    { value: "four_weeks", label: m.privacy_clear_range_month },
    { value: "everything", label: m.privacy_clear_range_all },
  ];
  const choices: ReadonlyArray<{
    kind: keyof ClearKinds;
    label: () => string;
    help: () => string;
  }> = [
    { kind: "history", label: m.privacy_clear_history, help: m.privacy_clear_history_help },
    { kind: "cookies", label: m.privacy_clear_cookies, help: m.privacy_clear_cookies_help },
    { kind: "cache", label: m.privacy_clear_cache, help: m.privacy_clear_cache_help },
    { kind: "downloads", label: m.privacy_clear_downloads, help: m.privacy_clear_downloads_help },
  ];

  let open = $state(false);
  let range = $state<BrowsingDataRange>("hour");
  let kinds = $state<ClearKinds>({ history: true, cookies: true, cache: true, downloads: false });
  let pending = $state(false);
  let failure = $state<"failed" | "private" | null>(null);
  let cleared = $state(false);
  let opener: HTMLElement | null = null;
  let chosen = $derived(Object.values(kinds).some(Boolean));

  async function clear() {
    const profile = tabs.profile()?.id;
    if (!profile || !chosen || pending) return;
    pending = true;
    failure = null;
    const outcome = await clearBrowsingData(profile, range, { ...kinds });
    pending = false;
    if (outcome === "cleared") {
      cleared = true;
      open = false;
    } else {
      failure = outcome;
    }
  }
</script>

<div data-setting="privacy.clear">
  <SettingsRow title={field.label()} description={field.description()}>
    <Button
      onclick={() => {
        opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
        cleared = false;
        failure = null;
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
            label={m.privacy_clear_range()}
            value={range}
            options={ranges.map((option) => ({ value: option.value, label: option.label() }))}
            onchange={(value) => (range = value as BrowsingDataRange)}
          />
          <div class="choices">
            {#each choices as choice (choice.kind)}
              <Checkbox
                label={choice.label()}
                description={choice.help()}
                checked={kinds[choice.kind]}
                onchange={(value) => (kinds[choice.kind] = value)}
              />
            {/each}
          </div>
          {#if failure}<p class="result error" role="alert">
              {failure === "private" ? m.privacy_clear_private() : m.privacy_clear_failed()}
            </p>{/if}
        </div>
        <footer>
          <Button onclick={() => (open = false)}>{m.action_cancel()}</Button>
          <Button type="submit" variant="primary" disabled={!chosen} {pending}
            >{m.privacy_clear_apply()}</Button
          >
        </footer>
      </form>
    </Dialog.Content>
  </Dialog.Portal>
</Dialog.Root>

<style>
  .choices {
    display: grid;
    gap: 14px;
  }

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
