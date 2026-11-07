<script lang="ts">
  import { Dialog } from "bits-ui";
  import { Globe02Icon } from "@hugeicons/core-free-icons";
  import type { SiteDataView } from "$shared/ipc/bindings";
  import { tabs } from "$domain/tabs";
  import { forgetAllSites, forgetSite, listSites } from "$domain/site-data";
  import * as m from "$shared/i18n/messages";
  import Button from "$shared/ui/Button";
  import EmptyState from "$shared/ui/EmptyState";
  import Icon from "$shared/ui/Icon";
  import SearchField from "$shared/ui/SearchField";
  import SettingsRow from "$shared/ui/SettingsRow";
  import { fields } from "../lib/catalog";

  const field = fields["privacy.site-data"];

  type View =
    | { state: "loading" }
    | { state: "failed" }
    | { state: "ready"; sites: SiteDataView[]; partial: boolean };

  let open = $state(false);
  let view = $state.raw<View>({ state: "loading" });
  let query = $state("");
  let removing = $state<string | null>(null);
  let failure = $state<{ site: string | null } | null>(null);
  let confirmingAll = $state(false);
  let clearingAll = $state(false);
  let opener: HTMLElement | null = null;
  let attempt = 0;

  let shown = $derived(
    view.state === "ready"
      ? view.sites.filter((site) => site.site.includes(query.trim().toLowerCase()))
      : [],
  );

  async function load() {
    const profile = tabs.profile()?.id;
    const current = ++attempt;
    view = { state: "loading" };
    const list = profile ? await listSites(profile) : null;
    if (current !== attempt) return;
    view = list ? { state: "ready", ...list } : { state: "failed" };
  }

  function show() {
    opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    query = "";
    failure = null;
    confirmingAll = false;
    open = true;
    void load();
  }

  async function remove(site: string) {
    const profile = tabs.profile()?.id;
    if (!profile || removing || clearingAll) return;
    removing = site;
    failure = null;
    const outcome = await forgetSite(profile, site);
    removing = null;
    if (outcome !== "cleared") {
      failure = { site };
      return;
    }
    if (view.state === "ready")
      view = { ...view, sites: view.sites.filter((entry) => entry.site !== site) };
  }

  async function removeAll() {
    const profile = tabs.profile()?.id;
    if (!profile || clearingAll || removing) return;
    clearingAll = true;
    failure = null;
    const outcome = await forgetAllSites(profile);
    clearingAll = false;
    confirmingAll = false;
    if (outcome === "cleared") await load();
    else failure = { site: null };
  }

  function kind(site: SiteDataView): string {
    if (site.cookies && site.storage) return m.privacy_clear_cookies();
    return site.cookies ? m.privacy_sites_kind_cookies() : m.privacy_sites_kind_storage();
  }
</script>

<div data-setting="privacy.site-data">
  <SettingsRow title={field.label()} description={field.description()}>
    <Button onclick={show}>{m.privacy_sites_open()}</Button>
  </SettingsRow>
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
        >{m.privacy_sites_description()}{#if view.state === "ready" && view.partial}
          {m.privacy_sites_partial()}{/if}</Dialog.Description
      >
      <div class="body">
        {#if view.state === "loading"}
          <p class="status" role="status">{m.privacy_sites_loading()}</p>
        {:else if view.state === "failed"}
          <div class="failed" role="alert">
            <p>{m.privacy_sites_load_failed()}</p>
            <Button onclick={() => void load()}>{m.surface_retry()}</Button>
          </div>
        {:else if view.sites.length === 0}
          <EmptyState title={m.privacy_sites_empty()} description={m.privacy_sites_empty_help()} />
        {:else}
          <SearchField
            label={m.privacy_sites_search()}
            placeholder={m.privacy_sites_search()}
            size="page"
            bind:value={query}
          />
          {#if shown.length === 0}
            <p class="status">{m.privacy_sites_no_match()}</p>
          {:else}
            <ul class="sites">
              {#each shown as site (site.site)}
                <li>
                  <span class="mark" aria-hidden="true"><Icon icon={Globe02Icon} size={15} /></span>
                  <span class="copy"><strong>{site.site}</strong><small>{kind(site)}</small></span>
                  <Button
                    size="compact"
                    variant="ghost"
                    aria-label={m.privacy_sites_remove({ site: site.site })}
                    pending={removing === site.site}
                    disabled={clearingAll || (removing !== null && removing !== site.site)}
                    onclick={() => void remove(site.site)}>{m.action_remove()}</Button
                  >
                </li>
              {/each}
            </ul>
          {/if}
        {/if}
        {#if failure}<p class="error" role="alert">
            {failure.site === null
              ? m.privacy_clear_failed()
              : m.privacy_sites_remove_failed({ site: failure.site })}
          </p>{/if}
        {#if confirmingAll}<p class="status">{m.privacy_sites_remove_all_confirm()}</p>{/if}
      </div>
      <footer>
        {#if confirmingAll}
          <Button disabled={clearingAll} onclick={() => (confirmingAll = false)}
            >{m.action_cancel()}</Button
          >
          <Button variant="danger" pending={clearingAll} onclick={() => void removeAll()}
            >{m.privacy_sites_remove_all_apply()}</Button
          >
        {:else}
          {#if view.state === "ready" && view.sites.length > 0}
            <Button
              variant="ghost"
              disabled={removing !== null}
              onclick={() => (confirmingAll = true)}>{m.privacy_sites_remove_all()}</Button
            >
          {/if}
          <Button onclick={() => (open = false)}>{m.panel_close()}</Button>
        {/if}
      </footer>
    </Dialog.Content>
  </Dialog.Portal>
</Dialog.Root>

<style>
  .body {
    display: grid;
    gap: 14px;
  }

  .status {
    margin: 0;
    color: var(--color-muted);
    font-size: var(--text-body);
    line-height: 1.6;
  }

  .failed {
    display: grid;
    justify-items: start;
    gap: 12px;
  }

  .failed p {
    margin: 0;
    color: var(--color-danger);
    font-size: var(--text-body);
  }

  .error {
    margin: 0;
    color: var(--color-danger);
    font-size: var(--text-body);
  }

  .sites {
    max-height: min(320px, 45vh);
    margin: 0;
    padding: 0;
    overflow-y: auto;
    list-style: none;
    border-radius: var(--radius-control);
    background: var(--color-surface);
  }

  .sites li {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 8px 8px 8px 12px;
  }

  .sites li + li {
    border-top: 1px solid var(--color-border);
  }

  .mark {
    display: grid;
    place-items: center;
    flex: none;
    width: 28px;
    height: 28px;
    border-radius: var(--radius-control);
    background: var(--color-fill);
    color: var(--color-muted);
  }

  .copy {
    display: grid;
    flex: 1;
    gap: 1px;
    min-width: 0;
  }

  .copy strong {
    overflow: hidden;
    font-size: var(--text-body);
    font-weight: 500;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .copy small {
    color: var(--color-muted);
    font-size: var(--text-label);
  }
</style>
