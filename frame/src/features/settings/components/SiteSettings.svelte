<script lang="ts">
  import { onMount } from "svelte";
  import * as m from "$shared/i18n/messages";
  import type {
    SitePermissionDecisionView,
    SitePermissionKindView,
    SitePermissionView,
    SitePermissionsView,
  } from "$shared/ipc/bindings";
  import { sitePermissions } from "$domain/permissions";
  import Button from "$shared/ui/Button";
  import Select from "$shared/ui/Select";

  const labels: Record<SitePermissionKindView, () => string> = {
    camera: m.pref_privacy_camera,
    microphone: m.pref_privacy_microphone,
    geolocation: m.pref_privacy_location,
    notifications: m.pref_privacy_notifications,
    clipboard_read: m.permission_clipboard,
    midi: m.site_permission_midi,
    sensors: m.site_permission_sensors,
    local_fonts: m.site_permission_local_fonts,
    automatic_downloads: m.permission_automatic_downloads,
    popups: m.permission_popups,
  };

  let view = $state<SitePermissionsView | null>(null);
  let loaded = $state(false);
  let busy = $state<string | null>(null);
  let failed = $state(false);

  let groups = $derived(
    (view?.kinds ?? [])
      .map((kind) => ({
        kind,
        entries: (view?.entries ?? []).filter((entry) => entry.kind === kind),
      }))
      .filter((group) => group.entries.length > 0),
  );

  const key = (entry: SitePermissionView) => `${entry.kind} ${entry.origin}`;

  // Pop-ups are blocked unless allowed, so they offer no Block choice.
  const decisions = (kind: SitePermissionKindView) =>
    kind === "popups"
      ? [{ value: "allow", label: m.permission_allow() }]
      : [
          { value: "allow", label: m.permission_allow() },
          { value: "block", label: m.permission_block() },
        ];

  async function load() {
    view = await sitePermissions.list();
    loaded = true;
  }

  async function change(entry: SitePermissionView, decision: SitePermissionDecisionView | null) {
    if (busy !== null) return;
    busy = key(entry);
    failed = !(await sitePermissions.set(entry.origin, entry.kind, decision));
    await load();
    busy = null;
  }

  onMount(() => void load());
</script>

<div class="site-settings" data-setting="privacy.site-settings" aria-busy={!loaded || undefined}>
  {#if loaded && view === null}
    <p class="note">{m.site_settings_unavailable()}</p>
  {:else if view?.private}
    <p class="note">{m.site_settings_private()}</p>
  {:else if loaded}
    {#if groups.length === 0}
      <p class="note">{m.site_settings_empty()}</p>
    {/if}
    {#each groups as group (group.kind)}
      <section aria-label={labels[group.kind]()}>
        <h3>{labels[group.kind]()}</h3>
        <ul>
          {#each group.entries as entry (key(entry))}
            <li>
              <span class="origin">{entry.origin}</span>
              <Select
                label={`${labels[entry.kind]()} · ${entry.origin}`}
                labelHidden
                options={decisions(entry.kind)}
                value={entry.decision}
                disabled={busy !== null}
                onchange={(value) => void change(entry, value as SitePermissionDecisionView)}
              />
              <Button
                variant="ghost"
                disabled={busy !== null}
                aria-label={`${m.action_remove()} ${entry.origin}`}
                onclick={() => void change(entry, null)}
              >
                {m.action_remove()}
              </Button>
            </li>
          {/each}
        </ul>
      </section>
    {/each}
    {#if !view?.complete}
      <p class="note">{m.site_settings_incomplete()}</p>
    {/if}
  {/if}
  {#if failed}
    <p class="note error" role="alert">{m.site_settings_failed()}</p>
  {/if}
</div>

<style>
  .site-settings {
    display: grid;
    gap: 14px;
    padding: 14px 16px;
  }

  h3 {
    margin: 0 0 4px;
    color: var(--color-muted);
    font-size: var(--text-caption);
    font-weight: 500;
  }

  ul {
    margin: 0;
    padding: 0;
    list-style: none;
  }

  li {
    display: flex;
    align-items: center;
    gap: 8px;
    min-block-size: 36px;
  }

  .origin {
    flex: 1;
    min-inline-size: 0;
    overflow: hidden;
    font-size: var(--text-body);
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .note {
    margin: 0;
    color: var(--color-muted);
    font-size: var(--text-body);
  }

  .error {
    color: var(--color-danger);
  }
</style>
