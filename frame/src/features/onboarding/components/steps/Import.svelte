<script lang="ts">
  import { Globe02Icon, Tick02Icon } from "@hugeicons/core-free-icons";
  import * as m from "$shared/i18n/messages";
  import { browserImport, type ImportKind } from "$domain/browser-import";
  import { preferences } from "$domain/preferences";
  import Button from "$shared/ui/Button";
  import Icon from "$shared/ui/Icon";
  import { BROWSERS } from "../../lib/catalog";
  import { browserMark } from "$shared/brand/browsers";
  import Mark from "$shared/ui/BrandMark";

  // What native found, or, until it can look, the browsers people come from.
  let sources = $derived(
    browserImport.found() ??
      BROWSERS.map(([id, name]) => ({
        id,
        browser: id,
        name,
        profiles: [],
        kinds: ["bookmarks", "history"] as ImportKind[],
        needsPermission: false,
        running: false,
      })),
  );
  let available = $derived(browserImport.available());
  let job = $derived(browserImport.current());
  let nothingFound = $derived(browserImport.found()?.length === 0);
  let failure = $derived(job?.kinds.find((entry) => entry.state === "failed") ?? null);
  let jobBrowser = $derived(sources.find((candidate) => candidate.id === job?.source)?.name ?? "");
  let chosen = $state<string | null>(null);
  let kinds = $state<ImportKind[]>(["essentials", "bookmarks", "history"]);
  let source = $derived(sources.find((candidate) => candidate.id === chosen) ?? null);
  let offered = $derived(kinds.filter((kind) => source?.kinds.includes(kind)));
  const kindLabel = (kind: ImportKind) =>
    kind === "essentials"
      ? m.import_essentials()
      : kind === "bookmarks"
        ? m.onb_import_bookmarks()
        : m.onb_import_history();
  const number = new Intl.NumberFormat();
  let total = $derived(job ? job.kinds.reduce((sum, entry) => sum + entry.done, 0) : 0);
  let share = $derived.by(() => {
    if (!job) return 0;
    const known = job.kinds.filter((entry) => entry.total);
    if (known.length === 0) return 0;
    return (
      known.reduce((sum, entry) => sum + entry.done, 0) /
      known.reduce((sum, entry) => sum + (entry.total ?? 0), 0)
    );
  });

  // The engines a search can go to without a URL of one's own; a custom
  // engine stays in Settings.
  const ENGINES = [
    ["google", "Google"],
    ["duckduckgo", "DuckDuckGo"],
    ["bing", "Bing"],
    ["brave", "Brave"],
  ] as const;
  let engine = $derived(preferences.value("search.engine"));
  let at = $derived(ENGINES.findIndex(([id]) => id === engine));
  // Each place is as wide as its name; the thumb takes the chosen one's
  // measure, read again whenever the track's size changes.
  let track = $state<HTMLElement>();
  let places = $state<HTMLButtonElement[]>([]);
  let thumb = $state<{ x: number; w: number } | null>(null);
  $effect(() => {
    const chosen = places[at];
    if (!track || !chosen) {
      thumb = null;
      return;
    }
    const measure = () => (thumb = { x: chosen.offsetLeft, w: chosen.offsetWidth });
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(track);
    return () => observer.disconnect();
  });

  // Why the chosen browser cannot be read yet, if it cannot.
  let hint = $derived.by(() => {
    if (!source || job) return null;
    if (!available) return m.onb_import_unavailable();
    if (source.running) return m.onb_import_quit({ browser: source.name });
    if (source.needsPermission) return m.onb_import_permission({ browser: source.name });
    return null;
  });

  function toggle(kind: ImportKind) {
    kinds = kinds.includes(kind)
      ? kinds.filter((candidate) => candidate !== kind)
      : [...kinds, kind];
  }
</script>

<!-- Access granted in System Settings tells nobody; look again on return. -->
<svelte:window
  onfocus={() => {
    if (sources.some((candidate) => candidate.needsPermission)) void browserImport.detect();
  }}
/>

<div class="import">
  <p class="label" aria-hidden="true">{m.onb_import_from()}</p>
  <div class="browsers" role="radiogroup" aria-label={m.onb_import_from()}>
    {#each sources as candidate, index (candidate.id)}
      <button
        type="button"
        class="browser"
        role="radio"
        aria-checked={chosen === candidate.id}
        disabled={browserImport.busy()}
        style:--i={index}
        onclick={() => (chosen = candidate.id)}
      >
        <span class="plate"
          >{#if browserMark(candidate.browser)}<Mark
              mark={browserMark(candidate.browser)!}
              size={30}
            />{:else}<Icon icon={Globe02Icon} size={22} />{/if}</span
        >
        <span class="name">{candidate.name}</span>
      </button>
    {/each}
  </div>

  <div class="next" aria-live="polite">
    {#if job?.finished && failure}
      <span class="reading"
        >{failure.problem === "busy"
          ? m.onb_import_quit({ browser: jobBrowser })
          : failure.problem === "permission"
            ? m.onb_import_permission({ browser: jobBrowser })
            : failure.problem === "storage"
              ? m.onb_import_storage()
              : m.onb_import_failed({ browser: jobBrowser })}</span
      >
      {#if failure.problem === "permission"}
        <Button
          variant="primary"
          size="compact"
          shape="capsule"
          onclick={() => void browserImport.openPermissionSettings(job.source)}
          >{m.onb_import_allow()}</Button
        >
      {/if}
    {:else if job?.finished && !job.cancelled && job.kinds.every((entry) => entry.total === 0)}
      <span class="reading">{m.import_kind_none()}</span>
    {:else if job?.finished}
      <span class="done"
        ><Icon icon={Tick02Icon} size={14} />{m.onb_import_done({
          count: number.format(total),
        })}</span
      >
    {:else if job}
      <span class="meter"><i style:transform={`scaleX(${share})`}></i></span>
      <span class="reading">{m.onb_import_reading({ count: number.format(total) })}</span>
      <button type="button" class="stop" onclick={() => void browserImport.cancel()}
        >{m.onb_import_stop()}</button
      >
    {:else if source}
      <div class="kinds">
        {#each source.kinds as kind (kind)}
          <button
            type="button"
            class="kind"
            aria-pressed={kinds.includes(kind)}
            onclick={() => toggle(kind)}>{kindLabel(kind)}</button
          >
        {/each}
      </div>
      {#if available && source.needsPermission}
        <Button
          variant="primary"
          size="compact"
          shape="capsule"
          onclick={() => void browserImport.openPermissionSettings(source.id)}
          >{m.onb_import_allow()}</Button
        >
      {:else}
        <Button
          variant="primary"
          size="compact"
          shape="capsule"
          disabled={!available || offered.length === 0 || source.running}
          onclick={() => void browserImport.start(source.id, source.profiles[0]?.id ?? "", offered)}
          >{m.onb_import_action()}</Button
        >
      {/if}
    {/if}
  </div>
  {#if nothingFound}
    <p class="note">{m.onb_import_none()}</p>
  {/if}
  {#if hint}<p class="note">{hint}</p>{/if}

  <div class="search">
    <p class="label" aria-hidden="true">{m.onb_search_with()}</p>
    <div class="engines" role="radiogroup" aria-label={m.onb_search_with()} bind:this={track}>
      {#if thumb}<span
          class="thumb"
          aria-hidden="true"
          style:width={`${thumb.w}px`}
          style:transform={`translateX(${thumb.x}px)`}
        ></span>{/if}
      {#each ENGINES as [id, label], index (id)}
        <button
          bind:this={places[index]}
          type="button"
          role="radio"
          aria-checked={engine === id}
          disabled={preferences.saving()}
          onclick={() => void preferences.set("search.engine", id)}>{label}</button
        >
      {/each}
    </div>
  </div>
</div>

<style>
  .import {
    display: grid;
    justify-items: start;
    gap: 26px;
    inline-size: 320px;
  }

  .label {
    margin: 0 0 -10px;
    color: var(--color-faint);
    font-size: 12.5px;
    font-weight: 500;
  }

  .search {
    display: grid;
    gap: 14px;
    inline-size: 100%;
    margin-block-start: 14px;
    animation: lift 640ms var(--ease-emphasized) 420ms backwards;
  }

  .search .label {
    margin: 0;
  }

  /* One control, four places, and a raised thumb that slides between
     them. */
  .engines {
    position: relative;
    display: flex;
    padding: 3px;
    border-radius: var(--radius-capsule);
    background: var(--color-fill);
  }

  /* Placed from the track's left edge in its own pixels, which do not
     mirror. */
  .thumb {
    position: absolute;
    inset-block: 3px;
    left: 0;
    border-radius: var(--radius-capsule);
    background: var(--color-raise-active);
    box-shadow: var(--shadow-raise);
    transition:
      transform var(--motion-slow) var(--ease-emphasized),
      width var(--motion-slow) var(--ease-emphasized);
  }

  .engines button {
    position: relative;
    flex: 1 1 auto;
    block-size: 30px;
    padding: 0 13px;
    border: 0;
    border-radius: var(--radius-capsule);
    background: none;
    color: var(--color-faint);
    font: inherit;
    font-size: 12px;
    font-weight: 500;
    white-space: nowrap;
    cursor: default;
    transition: color var(--motion-fast) var(--ease-out);
  }

  .engines button:hover {
    color: var(--color-muted);
  }

  .engines button[aria-checked="true"] {
    color: var(--color-text);
  }

  .engines button:focus-visible {
    outline: 2px solid var(--color-ring);
    outline-offset: 1px;
  }

  .browsers {
    display: grid;
    grid-template-columns: repeat(3, 64px);
    gap: 30px 22px;
  }

  .browser {
    display: grid;
    justify-items: center;
    gap: 9px;
    padding: 0;
    border: 0;
    background: none;
    color: var(--color-faint);
    font: inherit;
    font-size: 11.5px;
    font-weight: 500;
    cursor: default;
    animation: lift 640ms var(--ease-emphasized) backwards;
    animation-delay: calc(240ms + var(--i) * 50ms);
  }

  .plate {
    display: grid;
    place-items: center;
    inline-size: 64px;
    block-size: 64px;
    border-radius: var(--radius-card);
    background: var(--color-card);
    box-shadow: var(--shadow-raise);
    transition:
      background-color var(--motion-fast) var(--ease-out),
      box-shadow var(--motion-fast) var(--ease-out),
      scale var(--motion-slow) var(--ease-spring);
  }

  .browser:hover .plate {
    background: var(--color-fill-hover);
  }

  .browser:active .plate {
    scale: 0.94;
  }

  .browser:focus-visible {
    outline: none;
  }

  .browser:focus-visible .plate {
    outline: 2px solid var(--color-ring);
    outline-offset: 2px;
  }

  .browser[aria-checked="true"] {
    color: var(--color-text);
  }

  .browser[aria-checked="true"] .plate {
    background: var(--color-fill-active);
    box-shadow: inset 0 0 0 1.5px color-mix(in srgb, var(--color-lit) 75%, transparent);
  }

  .next {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 12px;
    min-block-size: 32px;
  }

  .kinds {
    display: flex;
    gap: 6px;
  }

  .kind {
    block-size: 28px;
    padding: 0 12px;
    border: 0;
    border-radius: var(--radius-capsule);
    background: transparent;
    box-shadow: inset 0 0 0 1px var(--color-border-strong);
    color: var(--color-faint);
    font: inherit;
    font-size: 12px;
    font-weight: 500;
    cursor: default;
    transition:
      background-color var(--motion-instant) var(--ease-smooth),
      color var(--motion-instant) var(--ease-smooth);
  }

  .kind[aria-pressed="true"] {
    background: var(--color-control);
    box-shadow: none;
    color: var(--color-text);
  }

  .meter {
    inline-size: 200px;
    block-size: 2px;
    overflow: hidden;
    border-radius: var(--radius-capsule);
    background: var(--color-fill-strong);
  }

  .meter i {
    display: block;
    block-size: 100%;
    background: var(--color-text);
    transform-origin: left center;
    transition: transform var(--motion-page) var(--ease-out);
  }

  .stop {
    padding: 4px 6px;
    border: 0;
    background: none;
    color: var(--color-faint);
    font: inherit;
    font-size: 12px;
    cursor: default;
  }

  .stop:hover {
    color: var(--color-muted);
  }

  .reading,
  .done,
  .note {
    color: var(--color-muted);
    font-size: 12.5px;
    font-variant-numeric: tabular-nums;
  }

  .done {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    color: var(--color-text);
  }

  .note {
    max-inline-size: 30ch;
    margin: -12px 0 0;
    color: var(--color-faint);
    line-height: 1.45;
  }

  @keyframes lift {
    from {
      opacity: 0;
      transform: translateY(10px);
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .browser,
    .search {
      animation: none;
    }

    .thumb {
      transition: none;
    }
  }
</style>
