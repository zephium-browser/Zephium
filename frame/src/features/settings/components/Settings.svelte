<script lang="ts">
  import LazyView from "$shared/ui/LazyView";
  import { tick, onMount } from "svelte";
  import * as m from "$shared/i18n/messages";
  import { preferences } from "$domain/preferences";
  import {
    sections,
    searchSettings,
    shownSections,
    type SettingsSection,
  } from "../lib/settings-model";
  import * as state from "../lib/settings-state.svelte";
  import NewTabPage from "./sections/NewTabPage.svelte";
  import Icon from "$shared/ui/Icon";
  import { ArrowRight01Icon, Search01Icon } from "@hugeicons/core-free-icons";
  import EmptyState from "$shared/ui/EmptyState";
  import Button from "$shared/ui/Button";
  const loaders = {
    appearance: () => import("./sections/AppearancePage.svelte"),
    general: () => import("./sections/GeneralPage.svelte"),
    tabs: () => import("./sections/TabsPage.svelte"),
    profiles: () => import("./sections/ProfilesPage.svelte"),
    search: () => import("./sections/SearchPage.svelte"),
    privacy: () => import("./sections/PrivacyPage.svelte"),
    shortcuts: () => import("./sections/KeyboardPage.svelte"),
    features: () => import("./sections/FeaturesPage.svelte"),
    focus: () => import("./sections/FocusPage.svelte"),
    work: () => import("./sections/WorkPage.svelte"),
    ai: () => import("./sections/AiPage.svelte"),
    skills: () => import("./sections/SkillsPage.svelte"),
    memory: () => import("./sections/MemoryPage.svelte"),
    sites: () => import("./sections/SitesPage.svelte"),
    mcp: () => import("./sections/ConnectionsPage.svelte"),
    performance: () => import("./sections/PerformancePage.svelte"),
    graphics: () => import("./sections/GraphicsPage.svelte"),
    account: () => import("./sections/AccountPage.svelte"),
    about: () => import("./sections/AboutPage.svelte"),
  };
  // A section this build does not offer opens the first one instead; one
  // that left with Work and AI opens the Work page, where they come back.
  let offered = $derived(shownSections());
  let current = $derived.by(() => {
    const wanted = sections.find((section) => section.id === state.section());
    return (
      offered.find((section) => section === wanted) ??
      offered.find((section) => wanted?.group === "work" && section.id === "work") ??
      offered[0]!
    );
  });
  let loader = $derived(current.id in loaders ? loaders[current.id as keyof typeof loaders] : null);
  let results = $derived(searchSettings(state.query()));
  let content: HTMLElement;
  let focusedRevision = -1;
  async function focusTarget() {
    const revision = state.selectionRevision();
    if (focusedRevision === revision) return;
    await tick();
    const target = state.highlighted();
    const row = target
      ? [...content.querySelectorAll<HTMLElement>("[data-setting]")].find(
          (element) => element.dataset.setting === target,
        )
      : null;
    if (target && !row) return;
    if (revision !== state.selectionRevision()) return;
    focusedRevision = revision;
    if (row) {
      content
        .querySelectorAll<HTMLElement>("[data-highlighted]")
        .forEach((element) => delete element.dataset.highlighted);
      row.dataset.highlighted = "true";
      row.tabIndex = -1;
      row.scrollIntoView({ block: "center" });
      row.focus();
    } else {
      content.scrollTop = 0;
      content.querySelector<HTMLElement>("h1")?.focus();
    }
  }
  $effect(() => {
    state.selectionRevision();
    state.section();
    state.highlighted();
    void focusTarget();
  });
  onMount(() => {
    const observer = new MutationObserver(() => {
      if (state.highlighted()) void focusTarget();
    });
    observer.observe(content, { childList: true, subtree: true });
    return () => observer.disconnect();
  });
</script>

<section class="settings-shell" aria-label={m.settings_title()}>
  <div class="settings-content scrolls" bind:this={content}>
    <div class="settings-page">
      {#key state.query() ? "search" : current.id}<div class="settings-view">
          <header class="settings-page-heading">
            <h1 tabindex="-1">
              {state.query() ? m.settings_search_results() : current.title()}
            </h1>
            {#if !state.query() && current.description()}<p>{current.description()}</p>{/if}
          </header>
          {#if preferences.saveFailed()}<div class="settings-feedback" role="alert">
              {m.settings_save_error()}
            </div>{/if}
          {#if state.query()}
            {#if results.length === 0}<EmptyState
                title={m.settings_no_results()}
                description={m.settings_no_results_desc()}
                >{#snippet icon()}<Icon
                    icon={Search01Icon}
                    size={24}
                  />{/snippet}{#snippet action()}<Button onclick={() => state.setQuery("")}
                    >{m.settings_clear_search()}</Button
                  >{/snippet}</EmptyState
              >
            {:else}<div class="settings-results">
                {#each results as result (result.id)}<button
                    type="button"
                    onclick={() => state.select(result.section as SettingsSection, result.target)}
                    ><span
                      ><span class="settings-result-section"
                        >{offered.find((section) => section.id === result.section)?.title()}</span
                      ><strong>{result.label()}</strong><small>{result.description()}</small></span
                    ><Icon icon={ArrowRight01Icon} size={16} /></button
                  >{/each}
              </div>{/if}
          {:else if current.id === "newtab"}<NewTabPage />
          {:else if loader}<LazyView
              {loader}
              loadingLabel={m.surface_loading()}
              failureLabel={m.surface_render_failed()}
              retryLabel={m.surface_retry()}>{#snippet children(View)}<View />{/snippet}</LazyView
            >{/if}
        </div>{/key}
      <span class="sr-only" role="status">{preferences.saving() ? m.settings_saving() : ""}</span>
    </div>
  </div>
</section>
