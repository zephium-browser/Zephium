<script lang="ts">
  import { untrack } from "svelte";
  import * as m from "$shared/i18n/messages";
  import type {
    SitePermissionDecisionView,
    SitePermissionKindView,
    SitePermissionsView,
  } from "$shared/ipc/bindings";
  import { sitePermissions } from "$domain/permissions";
  import Button from "$shared/ui/Button";
  import SegmentedControl from "$shared/ui/SegmentedControl";
  import {
    choiceOf,
    decisionOf,
    kindsInUse,
    shownKinds,
    type PermissionChoice,
  } from "../lib/site-info";

  /** `origin` is the site's canonical origin. It reads its decisions each time
   *  `open` turns true, and never while closed. Draws nothing while there is
   *  nothing to show. */
  let {
    origin,
    open,
    capture = null,
  }: {
    origin: string;
    open: boolean;
    capture?: { camera: string; microphone: string } | null;
  } = $props();

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
  const options = $derived([
    { value: "allow", label: m.permission_allow() },
    { value: "block", label: m.permission_block() },
  ]);

  let view = $state.raw<SitePermissionsView | null>(null);
  let loaded = $state(false);
  let busy = $state(false);
  let failed = $state(false);
  let decided = $state.raw<Partial<Record<SitePermissionKindView, SitePermissionDecisionView>>>({});
  let touched = $state.raw(new Set<SitePermissionKindView>());
  // What was just picked, shown until it is saved; a change that did not go
  // through puts the control back to what is.
  let choosing = $state.raw<Partial<Record<SitePermissionKindView, PermissionChoice>>>({});
  let section = $state<HTMLElement>();
  let latest = 0;

  let inUse = $derived(kindsInUse(capture));
  let shown = $derived(
    shownKinds(
      view?.kinds ?? [],
      new Set(Object.keys(decided) as SitePermissionKindView[]),
      inUse,
      touched,
    ),
  );
  let saved = $derived(Object.keys(decided).length > 0);
  let unreadable = $derived(loaded && view === null);
  let partial = $derived(view !== null && !view.complete);
  let present = $derived(shown.length > 0 || unreadable || partial || failed);

  async function load() {
    const mine = ++latest;
    const next = await sitePermissions.list(origin);
    if (mine !== latest) return;
    view = next;
    loaded = true;
    decided = Object.fromEntries(
      (next?.entries ?? [])
        .filter((entry) => entry.origin === origin)
        .map((entry) => [entry.kind, entry.decision]),
    );
  }

  $effect(() => {
    void origin;
    if (!open) return;
    untrack(() => {
      touched = new Set();
      failed = false;
      void load();
    });
  });

  async function change(kind: SitePermissionKindView, choice: PermissionChoice) {
    if (busy || choice === choiceOf(kind, decided[kind])) return;
    busy = true;
    failed = false;
    touched = new Set([...touched, kind]);
    choosing = { ...choosing, [kind]: choice };
    failed = !(await sitePermissions.set(origin, kind, decisionOf(kind, choice)));
    await load();
    choosing = {};
    busy = false;
  }

  async function reset() {
    if (busy) return;
    busy = true;
    failed = false;
    // The button that was pressed goes with the rows; the keyboard stays in
    // the panel around them.
    const panel = section?.closest<HTMLElement>("[data-tray-body]");
    for (const kind of Object.keys(decided) as SitePermissionKindView[]) {
      if (!(await sitePermissions.set(origin, kind, null))) failed = true;
    }
    touched = new Set();
    await load();
    busy = false;
    panel?.focus({ preventScroll: true });
  }
</script>

{#if present}
  <section
    class="permissions"
    bind:this={section}
    aria-label={m.site_info_permissions()}
    aria-busy={busy || undefined}
  >
    <header>
      <h3>{m.site_info_permissions()}</h3>
      {#if saved}
        <Button
          size="compact"
          variant="ghost"
          aria-label={m.site_info_permissions_reset()}
          disabled={busy}
          onclick={() => void reset()}
        >
          {m.zoom_reset()}
        </Button>
      {/if}
    </header>
    {#if unreadable}
      <p class="note">{m.site_settings_unavailable()}</p>
    {:else if shown.length > 0}
      <ul>
        {#each shown as kind (kind)}
          <li>
            <span class="name">
              <span aria-hidden="true">{labels[kind]()}</span>
              {#if inUse.has(kind)}<span class="use">{m.site_info_in_use()}</span>{/if}
            </span>
            <span class="control">
              <SegmentedControl
                label={labels[kind]()}
                size="compact"
                {options}
                value={choosing[kind] ?? choiceOf(kind, decided[kind])}
                onchange={(value) => void change(kind, value as PermissionChoice)}
              />
            </span>
          </li>
        {/each}
      </ul>
    {/if}
    {#if partial}
      <p class="note">{m.site_info_permissions_partial()}</p>
    {/if}
    {#if failed}
      <p class="note error" role="alert">{m.site_settings_failed()}</p>
    {/if}
  </section>
{/if}

<style>
  .permissions {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    outline: none;
  }

  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    min-block-size: 24px;
    padding-inline: var(--menu-item-inset);
  }

  h3 {
    margin: 0;
    color: var(--color-faint);
    font-size: var(--text-caption);
    font-weight: 550;
    letter-spacing: 0.01em;
  }

  ul {
    margin: 0;
    padding: 0;
    list-style: none;
  }

  /* The name and the choice, from the edge the panel's icons start at. The
     name takes what the choice leaves and may run to a second line; only
     when even its longest word has no room does the choice drop under it. */
  li {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0 8px;
    min-block-size: var(--menu-row);
    padding: 2px var(--menu-item-inset);
  }

  .name {
    display: grid;
    flex: 1 1 0;
    min-inline-size: min-content;
    font-size: var(--text-body);
    font-weight: 450;
    line-height: 16px;
    overflow-wrap: break-word;
    text-wrap: balance;
  }

  .control {
    display: flex;
    flex: none;
    margin-inline-start: auto;
  }

  /* Allow and Block, a little closer, so most names keep their line. */
  .control :global(.segment) {
    padding-inline: 6px;
    font-size: var(--text-label);
  }

  .use {
    color: var(--color-danger);
    font-size: var(--text-caption);
    line-height: 14px;
  }

  .note {
    margin: 0;
    padding: 0 var(--menu-item-inset);
    color: var(--color-muted);
    font-size: var(--text-label);
    line-height: 16px;
  }

  .error {
    color: var(--color-danger);
  }
</style>
