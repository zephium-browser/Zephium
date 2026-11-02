<script lang="ts">
  import { onMount } from "svelte";
  import { Alert02Icon, Globe02Icon, Tick02Icon } from "@hugeicons/core-free-icons";
  import * as m from "$shared/i18n/messages";
  import {
    browserImport,
    type ImportJob,
    type ImportKind,
    type ImportProblem,
    type ImportSource,
  } from "$domain/browser-import";
  import { browserMark } from "$shared/brand/browsers";
  import BrandMark from "$shared/ui/BrandMark";
  import SettingsRow from "$shared/ui/SettingsRow";
  import Select from "$shared/ui/Select";
  import Checkbox from "$shared/ui/Checkbox";
  import Button from "$shared/ui/Button";
  import Icon from "$shared/ui/Icon";

  const ALL_KINDS: readonly ImportKind[] = ["essentials", "bookmarks", "history"];

  let found = $derived(browserImport.found());
  let job = $derived(browserImport.current());
  let chosen = $state<string | null>(null);
  let profile = $state<string | null>(null);
  let kinds = $state<ImportKind[]>([...ALL_KINDS]);

  let source = $derived<ImportSource | null>(
    found?.find((candidate) => candidate.id === chosen) ?? found?.[0] ?? null,
  );
  let profileId = $derived(
    source?.profiles.find((candidate) => candidate.id === profile)?.id ??
      source?.profiles[0]?.id ??
      null,
  );
  let offered = $derived(kinds.filter((kind) => source?.kinds.includes(kind)));
  let running = $derived(browserImport.busy());
  let jobSource = $derived(found?.find((candidate) => candidate.id === job?.source) ?? null);
  let jobName = $derived(jobSource?.name ?? "");
  const number = new Intl.NumberFormat();

  onMount(() => {
    void browserImport.detect();
  });

  function choose(id: string) {
    chosen = id;
    profile = null;
  }

  // Arrow keys move the choice, as in any radio group.
  function step(event: KeyboardEvent, index: number) {
    const list = found ?? [];
    const delta =
      event.key === "ArrowRight" || event.key === "ArrowDown"
        ? 1
        : event.key === "ArrowLeft" || event.key === "ArrowUp"
          ? -1
          : 0;
    if (!delta || list.length < 2) return;
    event.preventDefault();
    const next = list[(index + delta + list.length) % list.length]!;
    choose(next.id);
    const group = (event.currentTarget as HTMLElement).parentElement;
    group?.querySelector<HTMLElement>(`[data-source="${CSS.escape(next.id)}"]`)?.focus();
  }

  function toggle(kind: ImportKind, on: boolean) {
    kinds = on ? [...kinds, kind] : kinds.filter((candidate) => candidate !== kind);
  }

  function kindName(kind: ImportKind) {
    if (kind === "essentials") return m.import_essentials();
    return kind === "bookmarks" ? m.onb_import_bookmarks() : m.onb_import_history();
  }

  function problemText(problem: ImportProblem | null | undefined, browser: string) {
    switch (problem) {
      case "busy":
        return m.onb_import_quit({ browser });
      case "permission":
        return m.onb_import_permission({ browser });
      case "storage":
        return m.onb_import_storage();
      default:
        return m.onb_import_failed({ browser });
    }
  }

  function progressText(entry: ImportJob["kinds"][number]) {
    switch (entry.state) {
      case "queued":
        return m.import_kind_waiting();
      case "running":
        return m.import_kind_reading();
      case "skipped":
        return m.import_kind_skipped();
      case "failed":
        return problemText(entry.problem, jobName);
      case "done":
        if (entry.done > 0) return m.import_kind_added({ count: number.format(entry.done) });
        return entry.total === 0 ? m.import_kind_none() : m.import_kind_current();
    }
  }

  let status = $derived.by(() => {
    if (found === null) return m.import_looking();
    if (found.length === 0) return m.onb_import_none();
    if (source?.needsPermission) return m.onb_import_permission({ browser: source.name });
    return m.pref_general_import_help();
  });
</script>

<!-- Access granted in System Settings tells nobody; look again on return. -->
<svelte:window
  onfocus={() => {
    if (!running && found?.some((candidate) => candidate.needsPermission))
      void browserImport.detect();
  }}
/>

{#snippet mark(id: string, size: number)}
  {@const art = browserMark(id)}
  {#if art}<BrandMark mark={art} {size} />{:else}<Icon
      icon={Globe02Icon}
      size={Math.round(size * 0.8)}
    />{/if}
{/snippet}

{#if job}
  <div class="job" data-setting="general.import" aria-live="polite">
    <div class="job-head">
      <span class="job-mark" aria-hidden="true"
        >{@render mark(jobSource?.browser ?? job.source, 22)}</span
      >
      <strong
        >{job.finished
          ? job.cancelled
            ? m.import_stopped()
            : m.import_imported({ browser: jobName })
          : m.import_importing({ browser: jobName })}</strong
      >
      {#if job.finished}
        {#if job.kinds.some((entry) => entry.problem === "permission")}
          <Button
            size="compact"
            onclick={() => void browserImport.openPermissionSettings(job.source)}
            >{m.onb_import_allow()}</Button
          >
        {/if}
        <Button size="compact" onclick={browserImport.dismiss}>{m.import_dismiss()}</Button>
      {:else}
        <Button size="compact" variant="ghost" onclick={() => void browserImport.cancel()}
          >{m.onb_import_stop()}</Button
        >
      {/if}
    </div>
    <ul>
      {#each job.kinds as entry (entry.kind)}
        <li data-state={entry.state}>
          <span class="state" aria-hidden="true"
            >{#if entry.state === "done"}<Icon
                icon={Tick02Icon}
                size={14}
              />{:else if entry.state === "failed"}<Icon icon={Alert02Icon} size={14} />{:else}<i
              ></i>{/if}</span
          >
          <span class="kind">{kindName(entry.kind)}</span>
          <span class="detail">{progressText(entry)}</span>
        </li>
      {/each}
    </ul>
  </div>
{:else}
  <SettingsRow settingId="general.import" title={m.pref_general_import()} description={status}>
    {#if source?.needsPermission}
      <Button size="compact" onclick={() => void browserImport.openPermissionSettings(source.id)}
        >{m.onb_import_allow()}</Button
      >
    {:else if source}
      <Button
        size="compact"
        variant="primary"
        disabled={running || offered.length === 0 || profileId === null}
        onclick={() => profileId && void browserImport.start(source.id, profileId, offered)}
        >{m.onb_import_action()}</Button
      >
    {/if}
  </SettingsRow>
  {#if found && found.length > 0}
    <div class="sources" role="radiogroup" aria-label={m.onb_import_from()}>
      {#each found as candidate, index (candidate.id)}
        {@const checked = candidate.id === source?.id}
        <button
          type="button"
          class="source"
          role="radio"
          aria-checked={checked}
          tabindex={checked ? 0 : -1}
          data-source={candidate.id}
          disabled={running}
          onclick={() => choose(candidate.id)}
          onkeydown={(event) => step(event, index)}
        >
          <span class="source-mark" aria-hidden="true">{@render mark(candidate.browser, 20)}</span>
          <span>{candidate.name}</span>
        </button>
      {/each}
    </div>
  {/if}
  {#if source && !source.needsPermission}
    {#if source.profiles.length > 1 && profileId}
      <SettingsRow title={m.import_profile()}>
        <Select
          label={m.import_profile()}
          labelHidden
          value={profileId}
          options={source.profiles.map((candidate) => ({
            value: candidate.id,
            label: candidate.name,
          }))}
          onchange={(value) => (profile = value)}
        />
      </SettingsRow>
    {/if}
    <SettingsRow title={m.import_what()}>
      <div class="kinds">
        {#each ALL_KINDS.filter((kind) => source.kinds.includes(kind)) as kind (kind)}
          <Checkbox
            label={kindName(kind)}
            checked={kinds.includes(kind)}
            onchange={(on) => toggle(kind, on)}
          />
        {/each}
      </div>
    </SettingsRow>
  {/if}
{/if}

<style>
  /* The browsers sit under the row that names them, as one choice. */
  .sources {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    padding: 0 18px 16px;
  }

  /* Rows draw their divider only against another row; the tiles sit between. */
  .sources + :global(.ui-settings-row)::before {
    content: "";
    position: absolute;
    inset-inline: 16px 0;
    top: 0;
    height: 1px;
    background: var(--color-border);
  }

  .source {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    height: 34px;
    padding: 0 14px 0 9px;
    border: 0;
    border-radius: var(--radius-control);
    background: transparent;
    box-shadow: inset 0 0 0 1px var(--color-border);
    color: var(--color-muted);
    font: inherit;
    font-size: var(--text-label);
    font-weight: 500;
    transition:
      background-color var(--motion-fast) var(--ease-smooth),
      box-shadow var(--motion-fast) var(--ease-smooth),
      color var(--motion-fast) var(--ease-smooth),
      scale var(--motion-slow) var(--ease-smooth);
  }

  .source:focus-visible {
    outline: 2px solid var(--color-ring);
    outline-offset: 2px;
  }

  .source:disabled {
    opacity: 0.5;
  }

  .source:hover:not(:disabled) {
    background: var(--row-hover);
    color: var(--color-text);
  }

  .source:active:not(:disabled) {
    scale: 0.97;
    transition-duration: var(--motion-fast);
  }

  .source[aria-checked="true"] {
    background: var(--color-fill-active);
    box-shadow: inset 0 0 0 1.5px color-mix(in srgb, var(--color-text) 55%, transparent);
    color: var(--color-text);
  }

  .source-mark,
  .job-mark {
    display: grid;
    place-items: center;
    color: var(--color-muted);
  }

  .kinds {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: flex-end;
    gap: 16px;
  }

  .job {
    display: grid;
    gap: 12px;
    padding: 16px 18px;
  }

  .job-head {
    display: flex;
    align-items: center;
    gap: 10px;
    min-height: 28px;
  }

  .job-head strong {
    flex: 1;
    min-width: 0;
    font-size: var(--text-body);
    font-weight: 550;
  }

  ul {
    display: grid;
    gap: 2px;
    margin: 0;
    padding: 0 0 0 32px;
    list-style: none;
  }

  li {
    display: grid;
    grid-template-columns: 18px minmax(0, 7rem) minmax(0, 1fr);
    align-items: center;
    gap: 8px;
    min-height: 26px;
    font-size: var(--text-label);
  }

  .state {
    display: grid;
    place-items: center;
    color: var(--color-muted);
  }

  .state i {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--color-faint);
  }

  .kind {
    color: var(--color-text);
  }

  .detail {
    color: var(--color-muted);
    overflow-wrap: anywhere;
  }

  li[data-state="running"] .state i {
    background: var(--color-accent);
    animation: pulse 1.1s var(--ease-smooth) infinite alternate;
  }

  li[data-state="done"] .state {
    color: var(--color-text);
  }

  /* The mark carries the problem; the sentence stays readable, not alarming. */
  li[data-state="failed"] .state {
    color: var(--color-danger);
  }

  @keyframes pulse {
    to {
      opacity: 0.35;
      scale: 0.7;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .source {
      transition: none;
    }

    .source:active:not(:disabled) {
      scale: none;
    }

    li[data-state="running"] .state i {
      animation: none;
    }
  }
</style>
