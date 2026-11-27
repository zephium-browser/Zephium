<script lang="ts">
  import { untrack } from "svelte";
  import { Clock01Icon } from "@hugeicons/core-free-icons";
  import { commands } from "$shared/ipc/bindings";
  import EmptyState from "$shared/ui/EmptyState";
  import Icon from "$shared/ui/Icon";
  import * as m from "$shared/i18n/messages";
  import { historySession } from "../lib/history-session.svelte";
  import ClosedTabs from "./ClosedTabs.svelte";
  import HistoryList from "./HistoryList.svelte";

  let {
    profile,
    query,
  }: {
    profile: string;
    query: string;
  } = $props();

  let session = $state.raw(untrack(() => historySession(profile, "sidebar")));

  // Only the owning profile decides which session this is. Starting one reads
  // its own state, so a tracked body would restart the session on every change
  // it made — including the query the reader just typed.
  $effect(() => {
    const owner = profile;
    return untrack(() => {
      const current = historySession(owner, "sidebar");
      session = current;
      void current.start(query);
      return () => current.stop();
    });
  });

  // The frame owns the search field; mirror its value into the session.
  $effect(() => {
    const value = query;
    untrack(() => {
      if (value !== session.query) session.search(value);
    });
  });

  const open = (url: string) => commands.browserOpenUrl(url, false);
</script>

{#if !query.trim()}<ClosedTabs {profile} />{/if}
{#if session.error}
  <EmptyState title={m.history_unavailable()} description={m.surface_retry()}>
    {#snippet icon()}<Icon icon={Clock01Icon} size={22} />{/snippet}
  </EmptyState>
{:else if session.empty}
  <EmptyState
    title={query.trim() ? m.history_no_matches() : m.browser_history_empty()}
    description={query.trim() ? m.history_no_matches_help() : m.history_empty_help()}
  >
    {#snippet icon()}<Icon icon={Clock01Icon} size={22} />{/snippet}
  </EmptyState>
{:else}
  <HistoryList {session} density="panel" onopen={(url) => void open(url)} />
{/if}
