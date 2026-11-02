<script lang="ts">
  import * as m from "$shared/i18n/messages";
  import { tabs } from "$domain/tabs";
  import type { TabCertificateProblem, TabFailure, TabView } from "$shared/ipc/bindings";
  import { commands } from "$shared/ipc/bindings";
  import Button from "$shared/ui/Button";

  let { tab, failure }: { tab: TabView; failure: TabFailure } = $props();
  let details = $state(false);

  let host = $derived.by(() => {
    try {
      return new URL(failure.url).host || failure.url;
    } catch {
      return failure.url;
    }
  });

  const help: Record<TabFailure["reason"], () => string> = {
    offline: m.navigation_error_offline,
    host_not_found: m.navigation_error_host_not_found,
    unreachable: m.navigation_error_unreachable,
    timed_out: m.navigation_error_timed_out,
    insecure: m.navigation_error_insecure,
    crashed: m.navigation_error_crashed,
    other: m.navigation_error_other,
  };

  const certificate: Record<TabCertificateProblem, () => string> = {
    expired: m.navigation_error_certificate_expired,
    untrusted: m.navigation_error_certificate_untrusted,
    name_mismatch: m.navigation_error_certificate_name_mismatch,
    revoked: m.navigation_error_certificate_revoked,
    other: m.navigation_error_certificate_other,
  };

  function continueUnsafe() {
    void commands.tabsContinuePastCertificate(tab.id).catch(() => {});
  }
</script>

<!--
  A page the person asked for did not load, or its process ended. Chrome says
  why in plain words and offers the same address again. A refused certificate
  says what was wrong with it; only a host on the person's own network has a
  way past, behind details and a warning.
-->
<div class="failed-page" role="alert">
  <h1>
    {failure.reason === "insecure"
      ? m.navigation_error_insecure_title({ host })
      : failure.reason === "crashed"
        ? m.navigation_error_crashed_title({ host })
        : m.navigation_error_title({ host })}
  </h1>
  <p>{help[failure.reason]()}</p>
  {#if failure.certificate}<p>{certificate[failure.certificate]()}</p>{/if}
  <Button onclick={() => tabs.navigate(tab.id, failure.url)}
    >{failure.reason === "crashed" ? m.navigation_error_reload() : m.surface_retry()}</Button
  >
  {#if failure.certificate && failure.can_continue}
    <Button
      variant="ghost"
      size="compact"
      aria-expanded={details}
      aria-controls="certificate-details"
      onclick={() => (details = !details)}
    >
      {details ? m.navigation_error_hide_details() : m.navigation_error_show_details()}
    </Button>
    {#if details}
      <div id="certificate-details" class="unsafe">
        <p>{m.navigation_error_certificate_unsafe()}</p>
        <Button variant="danger" onclick={continueUnsafe}>
          {m.navigation_error_certificate_continue({ host })}
        </Button>
      </div>
    {/if}
  {/if}
</div>

<style>
  .failed-page {
    display: grid;
    justify-items: center;
    align-content: center;
    gap: 12px;
    block-size: 100%;
    padding: 24px;
    text-align: center;
  }

  h1 {
    margin: 0;
    max-inline-size: 48ch;
    overflow-wrap: anywhere;
    color: var(--color-text);
    font-size: var(--text-title);
    font-weight: 500;
  }

  .unsafe {
    display: grid;
    justify-items: center;
    gap: 12px;
  }

  p {
    margin: 0;
    max-inline-size: 40ch;
    color: var(--color-muted);
    font-size: var(--text-body);
  }
</style>
