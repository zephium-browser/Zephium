<script lang="ts" module>
  // A reopened settings page still shares native's single permissions reader.
  let pending = Promise.resolve();
</script>

<script lang="ts">
  import * as m from "$shared/i18n/messages";
  import type {
    SitePermissionDecisionView,
    SitePermissionKindView,
    SitePermissionView,
    SitePermissionsView,
  } from "$shared/ipc/bindings";
  import { sitePermissions } from "$domain/permissions";
  import { tabs } from "$domain/tabs";
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
    downloads: m.permission_downloads,
    app_links: m.permission_app_links,
  };

  const activeProfile = $derived(tabs.profile()?.id);
  let view = $state.raw<SitePermissionsView | null>(null);
  let loadedFor = $state<string | null>(null);
  const loaded = $derived(loadedFor !== null && loadedFor === activeProfile);
  let busy = $state<string | null>(null);
  let failed = $state(false);
  let generation = 0;

  let groups = $derived(
    (view?.kinds ?? [])
      .map((kind) => ({
        kind,
        entries: (view?.entries ?? []).filter((entry) => entry.kind === kind),
      }))
      .filter((group) => group.entries.length > 0),
  );

  const key = (entry: SitePermissionView) => `${entry.kind} ${entry.origin} ${entry.scheme ?? ""}`;

  const scheme = (entry: SitePermissionView) =>
    entry.scheme === null ? null : m.site_settings_app_link_scheme({ scheme: entry.scheme });

  // The origin, and the scheme where a site has one grant per scheme, so each
  // row's controls are told apart.
  const subject = (entry: SitePermissionView) =>
    [entry.origin, scheme(entry)].filter(Boolean).join(" · ");

  // Pop-ups and links to apps are refused or asked about unless allowed, so
  // they offer no Block choice.
  const decisions = (kind: SitePermissionKindView) =>
    kind === "popups" || kind === "app_links"
      ? [{ value: "allow", label: m.permission_allow() }]
      : [
          { value: "allow", label: m.permission_allow() },
          { value: "block", label: m.permission_block() },
        ];

  const current = (profile: string, scope: number) =>
    profile === activeProfile && scope === generation;

  async function load(profile: string, scope: number) {
    if (!current(profile, scope)) return;
    const next = await sitePermissions.list();
    if (!current(profile, scope)) return;
    view = next;
    loadedFor = profile;
  }

  function change(entry: SitePermissionView, decision: SitePermissionDecisionView | null) {
    const profile = activeProfile;
    if (!profile || !loaded || busy !== null) return;
    const scope = generation;
    busy = key(entry);
    pending = (async () => {
      const applied = await sitePermissions
        .set(entry.origin, entry.kind, decision, entry.scheme)
        .catch(() => false);
      if (!current(profile, scope)) return;
      failed = !applied;
      await load(profile, scope);
      if (current(profile, scope)) busy = null;
    })();
  }

  $effect(() => {
    const profile = activeProfile;
    const scope = generation;
    loadedFor = null;
    view = null;
    busy = null;
    failed = false;
    if (profile) pending = pending.then(() => load(profile, scope));
    return () => {
      generation += 1;
    };
  });
</script>

<div class="site-settings" data-setting="privacy.site-settings" aria-busy={!loaded || undefined}>
  {#if loaded && view === null}
    <p class="note">{m.site_settings_unavailable()}</p>
  {:else if loaded && view?.private}
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
              <span class="name">
                <span class="origin">{entry.origin}</span>
                {#if entry.scheme !== null}<span class="scheme">{scheme(entry)}</span>{/if}
              </span>
              <Select
                label={`${labels[entry.kind]()} · ${subject(entry)}`}
                labelHidden
                options={decisions(entry.kind)}
                value={entry.decision}
                disabled={busy !== null}
                onchange={(value) => void change(entry, value as SitePermissionDecisionView)}
              />
              <Button
                variant="ghost"
                disabled={busy !== null}
                aria-label={`${m.action_remove()} ${subject(entry)}`}
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
  {#if loaded && failed}
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

  .name {
    display: grid;
    flex: 1;
    min-inline-size: 0;
  }

  .origin {
    overflow: hidden;
    font-size: var(--text-body);
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .scheme {
    color: var(--color-muted);
    font-size: var(--text-caption);
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
