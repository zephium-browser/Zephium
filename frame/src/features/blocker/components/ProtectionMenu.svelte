<script lang="ts">
  import type { Snippet } from "svelte";
  import {
    ArrowRight01Icon,
    Cancel01Icon,
    CursorPointer01Icon,
    ListViewIcon,
    ReloadIcon,
    ShieldBanIcon,
  } from "@hugeicons/core-free-icons";
  import * as m from "$shared/i18n/messages";
  import { blocker, blockerSites, hiding, shieldPresentation } from "$domain/blocker";
  import type { BlockerSiteAction } from "$shared/ipc/bindings";
  import Icon from "$shared/ui/Icon";
  import Switch from "$shared/ui/Switch";

  /** What blocking says and does for the page in front. `head` draws the first
   *  row; see BlockerShield. */
  let { head }: { head?: Snippet<[name: string | null, control?: Snippet]> } = $props();
  let status = $derived(blocker.status());
  let shield = $derived(shieldPresentation(status));
  let site = $derived(status.site);
  let on = $derived(status.applied_enabled === true);
  let busy = $state(false);
  let managing = $state(false);
  let failure = $state("");
  let contextKey = $derived(
    site ? `${site.context.profile}:${site.context.tab}:${site.context.site}` : "",
  );
  $effect(() => {
    void contextKey;
    failure = "";
    managing = false;
  });

  // What the page's own row cannot say: protection that is off, starting or in
  // trouble. A site paused is shown by its switch.
  let standing = $derived(
    status.protection === "disabled"
      ? m.site_info_blocking_off()
      : status.protection === "pending"
        ? m.site_info_blocking_starting()
        : status.protection === "degraded"
          ? m.site_info_blocking_attention()
          : "",
  );
  let present = $derived(
    shield.visible &&
      (head !== undefined ||
        standing !== "" ||
        status.desired_enabled !== true ||
        (site !== null && on) ||
        failure !== ""),
  );

  function ok(result: blocker.BlockerMutationResult) {
    return (
      result.state === "processed" &&
      (result.disposition.outcome === "applied" || result.disposition.outcome === "no_op")
    );
  }

  // The context's revision moves with every change, so each step reads it fresh.
  async function change(action: BlockerSiteAction): Promise<boolean> {
    const context = blocker.status().site?.context;
    if (!context) return false;
    const done = ok(await blockerSites.changeSite(context, action));
    if (!done) failure = m.site_info_blocking_failed();
    return done;
  }

  async function run(task: () => Promise<void>) {
    if (busy) return;
    busy = true;
    failure = "";
    try {
      await task();
    } finally {
      busy = false;
    }
  }

  const setProtected = (protect: boolean) =>
    run(async () => {
      if (!(await blockerSites.setProtected(protect))) failure = m.site_info_blocking_failed();
    });

  const enable = () =>
    run(async () => {
      if (!ok(await blocker.setEnabled(true))) failure = m.site_info_blocking_enable_failed();
    });

  const showAgain = (ids: string[]) =>
    run(async () => {
      for (const id of ids) if (!(await change({ kind: "remove_hide", id }))) return;
    });

  async function hide() {
    if (site && !(await hiding.start(site.context))) failure = m.site_info_hide_failed();
  }
</script>

{#snippet control()}
  {#if on && site?.ready}
    <Switch
      label={m.site_info_blocking()}
      labelHidden
      checked={!site.paused}
      disabled={busy || site.busy}
      onchange={(value) => void setProtected(value)}
    />
  {/if}
{/snippet}

{#if present}
  <section class="protection" data-tray-tools>
    {#if head}{@render head(m.site_info_blocking(), control)}{/if}

    {#if standing !== ""}
      <p
        class="standing"
        class:headed={head !== undefined}
        class:warning={shield.tone === "warning"}
      >
        {standing}
      </p>
    {/if}

    {#if status.desired_enabled !== true}
      <button
        type="button"
        class="ui-menu-item row"
        disabled={busy || !status.can_enable || status.preference !== "authoritative"}
        onclick={() => void enable()}
      >
        <span class="ui-menu-icon" aria-hidden="true"><Icon icon={ShieldBanIcon} size={16} /></span>
        <span class="label">{m.site_info_blocking_enable()}</span>
      </button>
    {/if}

    {#if site && on}
      {#if site.ready}
        <button
          type="button"
          class="ui-menu-item row"
          data-dismiss
          disabled={busy || site.busy}
          onclick={() => void hide()}
        >
          <span class="ui-menu-icon" aria-hidden="true"
            ><Icon icon={CursorPointer01Icon} size={16} /></span
          >
          <span class="label">{m.site_info_hide_elements()}</span>
        </button>
      {:else}
        <button
          type="button"
          class="ui-menu-item row"
          disabled={busy || site.busy}
          onclick={() => void run(async () => void (await change({ kind: "retry" })))}
        >
          <span class="ui-menu-icon" aria-hidden="true"><Icon icon={ReloadIcon} size={16} /></span>
          <span class="label">{m.site_info_blocking_retry()}</span>
        </button>
      {/if}
      {#if site.hides.length > 0}
        <button
          type="button"
          class="ui-menu-item row"
          aria-expanded={managing}
          onclick={() => (managing = !managing)}
        >
          <span class="ui-menu-icon" aria-hidden="true"><Icon icon={ListViewIcon} size={16} /></span
          >
          <span class="label">{m.site_info_hidden()}</span>
          <span class="count">{site.hides.length}</span>
          <span class="chevron" class:open={managing}
            ><Icon icon={ArrowRight01Icon} size={14} /></span
          >
        </button>
        {#if managing}
          <ul class="hides">
            {#each site.hides as hide (hide.id)}
              <li>
                <span class="name" title={hide.label}>{hide.label}</span>
                <button
                  type="button"
                  class="restore"
                  title={m.site_info_hidden_show({ name: hide.label })}
                  aria-label={m.site_info_hidden_show({ name: hide.label })}
                  disabled={busy || site.busy}
                  onclick={() => void showAgain([hide.id])}
                  ><Icon icon={Cancel01Icon} size={12} /></button
                >
              </li>
            {/each}
            {#if site.hides.length > 1}
              <li>
                <button
                  type="button"
                  class="all"
                  disabled={busy || site.busy}
                  onclick={() => void showAgain(site?.hides.map((hide) => hide.id) ?? [])}
                  >{m.site_info_hidden_show_all()}</button
                >
              </li>
            {/if}
          </ul>
        {/if}
      {/if}
    {/if}

    {#if failure}<p class="failure" role="status">{failure}</p>{/if}
  </section>
{/if}

<style>
  .protection {
    display: flex;
    flex-direction: column;
    min-inline-size: 0;
  }

  /* Under the page's own row, in its words' column; with no row, where every
     row's icon starts. */
  .standing {
    margin: 0 var(--menu-item-inset) 4px;
    color: var(--color-muted);
    font-size: var(--text-caption);
    line-height: 14px;
  }

  .standing.headed {
    margin-block-start: -4px;
    margin-inline-start: calc(var(--menu-item-inset) + 26px);
  }

  .standing.warning {
    color: var(--color-warning);
  }

  .row {
    inline-size: 100%;
    border: 0;
    background: transparent;
    font: inherit;
    font-size: var(--text-body);
    font-weight: 450;
    text-align: start;
  }

  .row:disabled {
    opacity: 0.45;
  }

  .row:focus-visible {
    background: var(--row-active);
    outline: 2px solid var(--color-ring);
    outline-offset: -2px;
  }

  .row:hover:not(:disabled) {
    background: var(--row-active);
  }

  .row:is(:hover, :focus-visible):not(:disabled) .ui-menu-icon {
    color: var(--color-text);
  }

  .label {
    flex: 1;
    min-inline-size: 0;
    text-wrap: balance;
  }

  .count {
    color: var(--color-muted);
    font-size: var(--text-caption);
    font-variant-numeric: tabular-nums;
  }

  .chevron {
    display: grid;
    color: var(--color-faint);
    transition: rotate var(--motion-fast) var(--ease-out);
  }

  .chevron.open {
    rotate: 90deg;
  }

  /* Aligned with the names above it, past the icon column. */
  .hides {
    max-block-size: 184px;
    margin: 0 0 2px;
    padding: 0;
    padding-inline-start: calc(var(--menu-item-inset) + 26px);
    overflow: auto;
    list-style: none;
  }

  .hides li {
    display: flex;
    align-items: center;
    gap: 6px;
    min-block-size: 28px;
    padding-inline-end: 4px;
  }

  .name {
    flex: 1;
    min-inline-size: 0;
    overflow: hidden;
    color: var(--color-text);
    font-size: var(--text-caption);
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .restore {
    display: grid;
    flex: none;
    place-items: center;
    inline-size: 22px;
    block-size: 22px;
    border: 0;
    border-radius: var(--radius-inset);
    background: transparent;
    color: var(--color-faint);
    cursor: default;
  }

  .restore:focus-visible {
    outline: 2px solid var(--color-ring);
    outline-offset: -2px;
  }

  .restore:hover:not(:disabled) {
    background: var(--row-hover);
    color: var(--color-text);
  }

  .all {
    padding: 2px 0;
    border: 0;
    background: transparent;
    color: var(--color-muted);
    font: inherit;
    font-size: var(--text-caption);
    cursor: default;
  }

  .all:focus-visible {
    outline: 2px solid var(--color-ring);
    outline-offset: -2px;
  }

  .all:hover:not(:disabled) {
    color: var(--color-text);
  }

  .failure {
    margin: 2px var(--menu-item-inset) 4px;
    color: var(--color-warning);
    font-size: var(--text-caption);
  }
</style>
