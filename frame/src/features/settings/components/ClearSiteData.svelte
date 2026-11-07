<script lang="ts">
  import { tabs } from "$domain/tabs";
  import { clearDataForPage, clearRequest } from "$domain/site-data";
  import * as notices from "$session/notice.svelte";
  import * as m from "$shared/i18n/messages";
  import Button from "$shared/ui/Button";
  import AccountDialog from "./account/AccountDialog.svelte";

  let request = $derived(clearRequest.pending());
  let host = $derived(request?.host ?? "");
  let pending = $state(false);
  let failure = $state<"failed" | "private" | null>(null);

  async function clear() {
    const target = request;
    const profile = tabs.profile()?.id;
    if (!target || !profile || pending) return;
    pending = true;
    failure = null;
    const outcome = await clearDataForPage(profile, target.url);
    pending = false;
    if (outcome === "cleared") {
      clearRequest.dismiss();
      notices.show(m.site_data_cleared({ site: target.host }), "data");
    } else {
      failure = outcome;
    }
  }
</script>

<AccountDialog
  bind:open={
    () => request !== null,
    (open) => {
      if (open || pending) return;
      failure = null;
      clearRequest.dismiss();
    }
  }
  title={m.site_data_clear_title({ site: host })}
  description={m.site_data_clear_description({ site: host })}
>
  <form
    onsubmit={(event) => {
      event.preventDefault();
      void clear();
    }}
  >
    {#if failure}<p class="error" role="alert">
        {failure === "private"
          ? m.privacy_clear_private()
          : m.site_data_clear_failed({ site: host })}
      </p>{/if}
    <footer>
      <Button disabled={pending} onclick={() => clearRequest.dismiss()}>{m.action_cancel()}</Button>
      <Button type="submit" variant="danger" {pending}>{m.privacy_clear_apply()}</Button>
    </footer>
  </form>
</AccountDialog>

<style>
  .error {
    margin: 0;
    color: var(--color-danger);
    font-size: var(--text-body);
  }
</style>
