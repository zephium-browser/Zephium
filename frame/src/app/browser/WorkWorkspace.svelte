<script lang="ts">
  import { untrack } from "svelte";
  import { tabs } from "$domain/tabs";
  import { preferences } from "$domain/preferences";
  import { surface } from "$domain/surface";
  import { settle } from "$domain/operations";
  import { environmentSession, type WorkEnvironmentSession } from "$domain/work-environment";
  import { loadWorkEnvironmentWorkspace } from "$features/work";
  import { commands } from "$shared/ipc/bindings";
  import LazyView from "$shared/ui/LazyView";
  import * as m from "$shared/i18n/messages";
  let session = $state.raw<WorkEnvironmentSession | null>(null);
  let navigationError = $state(false);
  // Tab state changes with every page load in any tab; the session follows
  // only the profile and space, which are strings and compare by value.
  let workProfile = $derived.by(() => {
    const profile = tabs.profile();
    return profile && profile.kind !== "incognito" ? profile.id : null;
  });
  let space = $derived(tabs.activeSpaceId());
  $effect(() => {
    const profile = workProfile;
    const current = profile && space ? untrack(() => environmentSession(profile, space)) : null;
    session = current;
    void current?.start(m.work_env_default_title());
    return () => current?.stopObserving();
  });
  async function browse(tab?: string) {
    const current = session;
    if (current && !(await current.flushView())) return;
    navigationError = false;
    try {
      const result = await settle(tab ? commands.tabsActivate(tab) : commands.tabsOpen());
      if (result.outcome === "applied" || result.outcome === "no_op") await surface.open(null);
      else if (result.outcome !== "deferred") navigationError = true;
    } catch {
      navigationError = true;
    }
  }
</script>

<section class="workspace" aria-label={m.mode_work()}>
  {#if session}{@const owner = session}{#key `${owner.profile}:${owner.space}`}
      <LazyView
        loader={loadWorkEnvironmentWorkspace}
        loadingLabel={m.surface_loading()}
        failureLabel={m.surface_render_failed()}
        retryLabel={m.surface_retry()}
        >{#snippet children(Environment)}<Environment
            session={owner}
            tabs={tabs.tabs()}
            spaceName={tabs.spaces().find((space) => space.id === owner.space)?.name ?? ""}
            profileLabel={tabs.profile()?.name ?? ""}
            currentTabId={tabs.activeId()}
            aiEnabled={preferences.value("ai.enabled") !== "false"}
            onopen={(id: string) => void browse(id)}
            onnewtab={() => void browse()}
          />{/snippet}</LazyView
      >
    {/key}{:else}<p>{m.work_regular_profile()}</p>{/if}
  {#if navigationError || surface.navigationFailed()}<p class="navigation-error" role="status">
      {m.work_env_navigation_failed()}
    </p>{/if}
</section>

<style>
  .workspace {
    position: relative;
    height: 100%;
    min-width: 0;
    min-height: 0;
    overflow: hidden;
    background: transparent;
  }

  .navigation-error {
    position: absolute;
    inset-inline: 24px;
    inset-block-end: 24px;
    margin: 0;
    padding: 12px;
    background: var(--color-surface);
    color: var(--color-text);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-control);
  }
</style>
