<script lang="ts">
  import * as m from "$shared/i18n/messages";
  import { tabs } from "$domain/tabs";
  import type { TabFailure, TabView } from "$shared/ipc/bindings";
  import Button from "$shared/ui/Button";

  let { tab, failure }: { tab: TabView; failure: TabFailure } = $props();

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
    other: m.navigation_error_other,
  };
</script>

<!--
  A page the person asked for did not load. Chrome says why in plain words and
  offers the same address again; a secure-connection failure has no way past.
-->
<div class="failed-page" role="alert">
  <h1>
    {failure.reason === "insecure"
      ? m.navigation_error_insecure_title({ host })
      : m.navigation_error_title({ host })}
  </h1>
  <p>{help[failure.reason]()}</p>
  <Button onclick={() => tabs.navigate(tab.id, failure.url)}>{m.surface_retry()}</Button>
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

  p {
    margin: 0;
    max-inline-size: 40ch;
    color: var(--color-muted);
    font-size: var(--text-body);
  }
</style>
