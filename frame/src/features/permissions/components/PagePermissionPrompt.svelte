<script lang="ts">
  import { Camera01Icon, Location01Icon, Mic01Icon } from "@hugeicons/core-free-icons";
  import { onMount } from "svelte";
  import type {
    PagePermissionPromptDecisionInput,
    PagePermissionPromptEntryView,
  } from "$shared/ipc/bindings";
  import * as m from "$shared/i18n/messages";
  import { pagePermissions as permissions } from "$domain/permissions";
  import Icon from "$shared/ui/Icon";
  import Button from "$shared/ui/Button";

  // A prompt that appears where the pointer already is must not take a click
  // aimed at what it covered or pushed down.
  const SETTLE_MS = 500;

  let { prompt }: { prompt: PagePermissionPromptEntryView } = $props();
  let dialog = $state<HTMLDivElement>();
  let denyButton = $state<HTMLButtonElement>();
  const shownAt = performance.now();
  let busy = $derived(permissions.busy());
  let failure = $derived(permissions.failure());
  let camera = $derived(prompt.kinds.includes("camera"));
  let microphone = $derived(prompt.kinds.includes("microphone"));
  let location = $derived(prompt.kinds.includes("geolocation"));
  // The exact origin names the site, as Chrome does: only a secure scheme
  // goes unsaid; http and any port stay in view.
  let site = $derived(
    prompt.origin.startsWith("https://") ? prompt.origin.slice("https://".length) : prompt.origin,
  );
  let title = $derived(
    location
      ? m.page_permission_title_location({ site })
      : camera && microphone
        ? m.page_permission_title_both({ site })
        : camera
          ? m.page_permission_title_camera({ site })
          : m.page_permission_title_microphone({ site }),
  );
  let icon = $derived(location ? Location01Icon : camera ? Camera01Icon : Mic01Icon);
  // macOS gives no position to a browser the person turned it off for.
  let systemOff = $derived(location && prompt.blocked_by_system);
  let described = $derived(
    [systemOff && "page-permission-system", !prompt.rememberable && "page-permission-private"]
      .filter(Boolean)
      .join(" ") || undefined,
  );

  // The card leaves a person typing in chrome where they are; focus in a
  // page view stays there whatever chrome focuses.
  function typing() {
    const active = document.activeElement;
    return (
      active instanceof HTMLInputElement ||
      active instanceof HTMLTextAreaElement ||
      (active instanceof HTMLElement && active.isContentEditable)
    );
  }

  onMount(() =>
    queueMicrotask(() => {
      if (!prompt.in_sidebar || !typing()) denyButton?.focus();
    }),
  );

  function respond(decision: PagePermissionPromptDecisionInput) {
    if (busy) return;
    if (decision !== "dismiss" && performance.now() - shownAt < SETTLE_MS) return;
    permissions.respond(prompt, decision);
  }

  function handleWindowKeydown(event: KeyboardEvent) {
    // The card answers keys only while focus is in it; the sheet holds all.
    if (prompt.in_sidebar && !dialog?.contains(document.activeElement)) return;
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      // Closing the prompt decides nothing beyond this request.
      if (!busy) respond("dismiss");
      return;
    }
    if (prompt.in_sidebar || event.key !== "Tab") return;
    event.stopPropagation();
    const buttons = Array.from(dialog?.querySelectorAll("button") ?? []);
    const first = buttons.at(0);
    const last = buttons.at(-1);
    if (first === undefined || last === undefined) return;
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  }
</script>

<svelte:window onkeydown={handleWindowKeydown} />

{#snippet status()}
  {#if failure !== null}
    <p role="alert" class="text-[10.5px] leading-4 text-danger">{failure}</p>
  {:else if busy}
    <p role="status" class="text-[10.5px] leading-4 text-muted">
      {m.page_permission_applying()}
    </p>
  {/if}
{/snippet}

{#snippet choices(size: "compact" | "regular")}
  {#if prompt.rememberable}
    <div class="flex flex-col gap-1.5">
      <Button
        variant="secondary"
        {size}
        class="w-full"
        disabled={busy}
        aria-busy={busy || undefined}
        onclick={() => respond("always_allow")}
      >
        {m.page_permission_allow_always()}
      </Button>
      <Button
        variant="secondary"
        {size}
        class="w-full"
        disabled={busy}
        aria-busy={busy || undefined}
        onclick={() => respond("allow_once")}
      >
        {m.page_permission_allow_once()}
      </Button>
      <Button
        bind:ref={denyButton}
        variant="secondary"
        {size}
        class="w-full"
        disabled={busy}
        aria-busy={busy || undefined}
        onclick={() => respond("always_deny")}
      >
        {m.page_permission_deny_always()}
      </Button>
    </div>
  {:else}
    <div class="flex justify-end gap-2">
      <Button
        bind:ref={denyButton}
        variant="secondary"
        {size}
        disabled={busy}
        aria-busy={busy || undefined}
        onclick={() => respond("deny_once")}
      >
        {m.page_permission_deny()}
      </Button>
      <Button
        variant="primary"
        {size}
        disabled={busy}
        aria-busy={busy || undefined}
        onclick={() => respond("allow_once")}
      >
        {m.page_permission_allow_once()}
      </Button>
    </div>
  {/if}
{/snippet}

{#if prompt.in_sidebar}
  <!-- Under the address field, in chrome no page view overlaps: the page
       stays on screen and the rest of the chrome stays usable. -->
  <div
    bind:this={dialog}
    role="alertdialog"
    aria-labelledby="page-permission-title"
    aria-describedby={described}
    class="page-permission"
    data-zephium-page-permission-prompt
  >
    <div class="head">
      <Icon {icon} size={14} />
      <p id="page-permission-title" class="title">{title}</p>
    </div>
    {#if systemOff}
      <p id="page-permission-system" class="detail">{m.page_permission_location_off()}</p>
    {/if}
    {#if !prompt.rememberable}
      <p id="page-permission-private" class="detail">{m.page_permission_private()}</p>
    {/if}
    {@render status()}
    {@render choices("compact")}
  </div>
{:else}
  <div class="fixed inset-0 z-50 flex items-center justify-center bg-canvas/80 p-4">
    <div
      bind:this={dialog}
      role="alertdialog"
      aria-modal="true"
      aria-labelledby="page-permission-title"
      aria-describedby={described}
      class="w-[min(360px,calc(100vw-32px))] rounded-panel border border-border-strong bg-raised p-4 text-start shadow-[var(--shadow-overlay)]"
      data-zephium-page-permission-prompt
    >
      <div class="flex items-center gap-3">
        <span
          class="flex h-9 w-9 shrink-0 items-center justify-center rounded-control-compact bg-accent-soft text-accent"
          aria-hidden="true"
        >
          <Icon {icon} size={19} />
        </span>
        <h1
          id="page-permission-title"
          class="min-w-0 flex-1 text-[14px] leading-5 font-semibold [overflow-wrap:anywhere] text-text"
        >
          {title}
        </h1>
      </div>

      {#if systemOff}
        <p id="page-permission-system" class="mt-3 text-[11.5px] leading-4 text-muted">
          {m.page_permission_location_off()}
        </p>
      {/if}

      {#if !prompt.rememberable}
        <p id="page-permission-private" class="mt-3 text-[11.5px] leading-4 text-muted">
          {m.page_permission_private()}
        </p>
      {/if}

      {#if failure !== null || busy}<div class="mt-2.5">{@render status()}</div>{/if}
      <div class="mt-4">{@render choices("regular")}</div>
    </div>
  </div>
{/if}

<style>
  .page-permission {
    display: grid;
    gap: 8px;
    margin-block-end: 8px;
    padding: 10px;
    border-radius: var(--radius-row);
    background: var(--color-fill);
    color: var(--color-text);
  }

  .head {
    display: flex;
    gap: 8px;
    align-items: flex-start;
  }

  .head > :global(svg) {
    flex: none;
    margin-block-start: 2px;
    color: var(--color-muted);
  }

  .title {
    min-inline-size: 0;
    margin: 0;
    font-size: 12px;
    font-weight: 500;
    line-height: 16px;
    overflow-wrap: anywhere;
  }

  .detail {
    margin: 0;
    color: var(--color-muted);
    font-size: 11px;
    line-height: 14px;
    overflow-wrap: anywhere;
  }
</style>
