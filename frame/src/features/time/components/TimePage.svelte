<!--
  Time as a page: a day or a week, where it went site by site, one site in
  depth, and focus with the sites it shuts.
-->
<script lang="ts">
  import { untrack } from "svelte";
  import {
    ArrowDown01Icon,
    ArrowLeft01Icon,
    ArrowLeft02Icon,
    ArrowRight01Icon,
    ArrowUp01Icon,
    Clock01Icon,
    Globe02Icon,
    LinkSquare02Icon,
    LockIcon,
  } from "@hugeicons/core-free-icons";
  import * as m from "$shared/i18n/messages";
  import { reducedMotion } from "$shared/lib/motion";
  import { commands } from "$shared/ipc/bindings";
  import { favicons } from "$domain/favicons";
  import { preferences } from "$domain/preferences";
  import { surface as browser } from "$domain/surface";
  import { tabs } from "$domain/tabs";
  import {
    TimeSession,
    currentPeriod,
    dayLabel,
    duration,
    hourLabel,
    liveBucket,
    weekLabel,
    weekdayShort,
    type Span,
  } from "$domain/time";
  import Bars from "$shared/ui/data/Bars";
  import Button from "$shared/ui/Button";
  import EmptyState from "$shared/ui/EmptyState";
  import FavIcon from "$shared/ui/FavIcon";
  import Icon from "$shared/ui/Icon";
  import IconButton from "$shared/ui/IconButton";
  import SegmentedControl from "$shared/ui/SegmentedControl";
  import {
    STEADY_SECONDS,
    columns,
    comparison,
    dailyAverage,
    describeBucket,
    totals,
  } from "../lib/overview";
  import FocusCard from "./FocusCard.svelte";
  import ShutSites from "./ShutSites.svelte";
  import SiteList from "./SiteList.svelte";

  let profile = $derived(tabs.profile()?.id ?? "unbound");
  let privateWindow = $derived(tabs.profile()?.kind === "incognito");
  let session = $state.raw(untrack(() => new TimeSession(profile, "day")));
  let pointed = $state<string | null>(null);

  $effect(() => {
    const owner = profile;
    return untrack(() => {
      const current = new TimeSession(owner, "day");
      session = current;
      current.start();
      return () => current.stop();
    });
  });

  let tracking = $derived(preferences.value("time.track") !== "false");
  let period = $derived(session.period);
  let overview = $derived(session.overview);
  let report = $derived(session.report);
  let site = $derived(session.site);
  let whole = $derived(totals(overview?.buckets ?? []));
  let sum = $derived(totals(report?.buckets ?? []));
  let values = $derived(columns(report?.buckets ?? []));
  let live = $derived(liveBucket(period));
  let change = $derived(report && site === null ? comparison(period, sum, report.previous) : null);
  let average = $derived(site === null ? dailyAverage(period, sum) : null);
  let highlight = $derived.by(() => {
    if (site !== null) return null;
    const series = overview?.sites.find((entry) => entry.site === pointed)?.series ?? [];
    return series.length > 0 ? series : null;
  });
  let ticks = $derived(
    period.span === "day"
      ? [0, 3, 6, 9, 12, 15, 18, 21].map((index) => ({ index, label: hourLabel(index) }))
      : values.map((_, index) => ({ index, label: weekdayShort(period.start + index) })),
  );
  let periodLabel = $derived.by(() => {
    const now = currentPeriod(period.span);
    const back = (now.start - period.start) / (period.span === "day" ? 1 : 7);
    if (period.span === "day") {
      return back === 0 ? m.time_today() : back === 1 ? m.time_yesterday() : dayLabel(period.start);
    }
    return back === 0
      ? m.time_this_week()
      : back === 1
        ? m.time_last_week()
        : weekLabel(period.start);
  });
  let changeLabel = $derived.by(() => {
    if (change === null) return null;
    const amount = duration(Math.abs(change));
    const day = period.span === "day";
    if (Math.abs(change) < STEADY_SECONDS) return day ? m.time_steady_day() : m.time_steady_week();
    if (change < 0)
      return day ? m.time_less_day({ duration: amount }) : m.time_less_week({ duration: amount });
    return day ? m.time_more_day({ duration: amount }) : m.time_more_week({ duration: amount });
  });
  let focusSum = $derived(
    session.focus.reduce(
      (total, day) => ({
        seconds: total.seconds + day.seconds,
        sessions: total.sessions + day.sessions,
        completed: total.completed + day.completed,
      }),
      { seconds: 0, sessions: 0, completed: 0 },
    ),
  );
  let suggestions = $derived(
    (overview?.sites ?? []).map((entry) => ({ site: entry.site, icon: entry.icon })),
  );
  let siteEntry = $derived(overview?.sites.find((entry) => entry.site === site) ?? null);
  let siteMark = $derived(site ? favicons.mark(siteEntry?.icon, `https://${site}/`) : null);
  let shut = $derived(
    site !== null &&
      preferences
        .value("focus.blocked")
        .split("\n")
        .some((entry) => entry.length > 0 && (site === entry || site.endsWith(`.${entry}`))),
  );

  let shutSection: HTMLElement | undefined = $state();

  function toggleShut() {
    if (site === null) return;
    const list = preferences
      .value("focus.blocked")
      .split("\n")
      .filter((entry) => entry.length > 0);
    const next = shut
      ? list.filter((entry) => !(site === entry || site.endsWith(`.${entry}`)))
      : [...list, site];
    void preferences.set("focus.blocked", next.join("\n"));
  }
</script>

<section class="library-shell">
  <header class="internal-toolbar">
    <IconButton
      icon={ArrowLeft02Icon}
      label={m.settings_back()}
      onclick={() => void browser.close()}
    /><span class="toolbar-current">{m.time_title()}</span>
  </header>

  <div class="scroller">
    {#if privateWindow}
      <EmptyState title={m.time_private()} description={m.time_local_only()}>
        {#snippet icon()}<Icon icon={LockIcon} size={28} />{/snippet}
      </EmptyState>
    {:else if !tracking}
      <EmptyState title={m.time_off_title()} description={m.time_off_help()}>
        {#snippet icon()}<Icon icon={Clock01Icon} size={28} />{/snippet}
        {#snippet action()}<Button
            variant="primary"
            shape="capsule"
            onclick={() => void preferences.set("time.track", "true")}>{m.time_turn_on()}</Button
          >{/snippet}
      </EmptyState>
    {:else}
      <div class="column">
        <div class="controls">
          <SegmentedControl
            label={m.time_span()}
            size="compact"
            value={period.span}
            options={[
              { value: "day", label: m.time_span_day() },
              { value: "week", label: m.time_span_week() },
            ]}
            onchange={(value) => session.show(value as Span)}
          />
          <div class="stepper">
            <IconButton
              icon={ArrowLeft01Icon}
              label={period.span === "day" ? m.time_previous_day() : m.time_previous_week()}
              size={15}
              onclick={() => session.shift(-1)}
            />
            <span class="period">{periodLabel}</span>
            <IconButton
              icon={ArrowRight01Icon}
              label={period.span === "day" ? m.time_next_day() : m.time_next_week()}
              size={15}
              disabled={session.current}
              onclick={() => session.shift(1)}
            />
          </div>
          {#if !session.current}
            <Button size="compact" shape="capsule" variant="ghost" onclick={() => session.today()}
              >{m.time_go_today()}</Button
            >
          {/if}
        </div>

        <div class="layout">
          <div class="main">
            <section class="card overview">
              {#if site !== null}
                <div class="site-head">
                  <button type="button" class="crumb" onclick={() => session.select(null)}
                    ><Icon icon={ArrowLeft01Icon} size={13} />{m.time_all_sites()}</button
                  >
                  <div class="site-title">
                    <FavIcon
                      image={siteMark?.image ?? null}
                      tone={siteMark?.tone}
                      size={22}
                      lit
                      fallback={Globe02Icon}
                    />
                    <h2>{site}</h2>
                  </div>
                </div>
              {/if}
              <div class="figures">
                <div class="figure">
                  <p class="total">{duration(sum.all)}</p>
                  <p class="sub">
                    {#if site !== null}{m.time_site_total({
                        site,
                      })}{:else}{m.time_on_web()}{#if sum.work >= 60}<span
                          class="dot"
                          aria-hidden="true">·</span
                        >{m.time_in_work({ duration: duration(sum.work) })}{/if}{/if}
                  </p>
                </div>
                {#if changeLabel !== null}
                  <p class="change">
                    {#if change !== null && Math.abs(change) >= STEADY_SECONDS}<Icon
                        icon={change < 0 ? ArrowDown01Icon : ArrowUp01Icon}
                        size={12}
                      />{/if}{changeLabel}
                  </p>
                {:else if average !== null}
                  <p class="change">{m.time_daily_average({ duration: duration(average) })}</p>
                {/if}
              </div>
              <Bars
                {values}
                {highlight}
                live={session.current ? live : -1}
                {average}
                {ticks}
                height={196}
                label={period.span === "day" ? m.time_chart_day() : m.time_chart_week()}
                describe={(index) => describeBucket(period, index)}
                format={duration}
                onselect={period.span === "week"
                  ? (index) => session.view({ span: "day", start: period.start + index })
                  : undefined}
              />
              {#if site !== null}
                <div class="site-actions">
                  <Button
                    size="compact"
                    shape="capsule"
                    onclick={() => void commands.browserOpenUrl(`https://${site}/`, true)}
                    ><Icon icon={LinkSquare02Icon} size={14} />{m.time_open_site()}</Button
                  >
                  <Button
                    size="compact"
                    shape="capsule"
                    variant={shut ? "secondary" : "ghost"}
                    aria-pressed={shut}
                    disabled={preferences.saving()}
                    onclick={toggleShut}>{m.focus_shut_site()}</Button
                  >
                </div>
              {/if}
            </section>

            {#if session.failed}
              <p class="quiet">{m.time_unavailable()}</p>
            {:else if overview && overview.sites.length > 0}
              <section class="card sites">
                <SiteList
                  sites={overview.sites}
                  total={whole.browse}
                  size="page"
                  selected={site}
                  onpoint={(entry) => (pointed = entry)}
                  onselect={(entry) => session.select(entry === site ? null : entry)}
                />
              </section>
            {:else if overview}
              <section class="card">
                <EmptyState title={m.time_no_sites()} description={m.time_no_sites_help()}>
                  {#snippet icon()}<Icon icon={Globe02Icon} size={24} />{/snippet}
                </EmptyState>
              </section>
            {/if}
          </div>

          <aside class="side">
            <FocusCard
              size="page"
              onmanage={() =>
                shutSection?.scrollIntoView({
                  behavior: reducedMotion() ? "instant" : "smooth",
                  block: "start",
                })}
            />
            {#if session.focus.length > 0}
              <section class="card stats" aria-label={m.focus_title()}>
                <div>
                  <span class="stat">{duration(focusSum.seconds)}</span>
                  <span class="label">{m.focus_focused()}</span>
                </div>
                <div>
                  <span class="stat">{focusSum.sessions}</span>
                  <span class="label">{m.focus_sessions()}</span>
                </div>
                <div>
                  <span class="stat">{focusSum.completed}</span>
                  <span class="label">{m.focus_completed()}</span>
                </div>
              </section>
            {/if}
            <section class="card shut" bind:this={shutSection}>
              <h3>{m.focus_shut_title()}</h3>
              <p class="help">{m.focus_shut_help()}</p>
              <ShutSites {suggestions} />
            </section>
            <p class="local"><Icon icon={LockIcon} size={11} />{m.time_local_only()}</p>
          </aside>
        </div>
      </div>
    {/if}
  </div>
</section>

<style>
  .scroller {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    scrollbar-gutter: stable;
  }

  .column {
    --page-gutter: clamp(16px, 4vw, 48px);

    container: time-column / inline-size;
    display: grid;
    gap: 16px;
    max-inline-size: 1080px;
    margin-inline: auto;
    padding: 4px var(--page-gutter) 48px;
  }

  .controls {
    display: flex;
    align-items: center;
    gap: 12px;
    min-height: 34px;
  }

  .stepper {
    display: flex;
    align-items: center;
    gap: 2px;
  }

  .period {
    min-inline-size: 120px;
    font-size: var(--text-body);
    font-weight: 600;
    text-align: center;
  }

  .layout {
    display: grid;
    grid-template-columns: minmax(0, 1fr) clamp(260px, 32%, 320px);
    align-items: start;
    gap: 16px;
  }

  @container time-column (width < 600px) {
    .layout {
      grid-template-columns: minmax(0, 1fr);
    }
  }

  .main,
  .side {
    display: grid;
    gap: 16px;
    min-width: 0;
  }

  .card {
    padding: 20px;
    border-radius: var(--radius-card);
    background: var(--color-card);
    box-shadow: var(--row-rim);
  }

  .overview {
    display: grid;
    gap: 22px;
    padding-block-start: 22px;
  }

  .site-head {
    display: grid;
    gap: 10px;
    animation: enter var(--motion-slow) var(--ease-out);
  }

  .crumb {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    justify-self: start;
    padding: 2px 8px 2px 4px;
    border: 0;
    border-radius: var(--radius-capsule);
    background: transparent;
    color: var(--color-muted);
    font: inherit;
    font-size: var(--text-label);
    cursor: pointer;
  }

  .crumb:hover {
    background: var(--color-fill);
    color: var(--color-text);
  }

  .crumb:focus-visible {
    outline: 2px solid var(--color-ring);
  }

  .site-title {
    display: flex;
    align-items: center;
    gap: 10px;
  }

  h2 {
    margin: 0;
    font-size: var(--text-object-title);
    font-weight: 600;
  }

  .figures {
    display: flex;
    align-items: flex-end;
    justify-content: space-between;
    gap: 16px;
    flex-wrap: wrap;
  }

  .figure {
    display: grid;
    gap: 4px;
  }

  .total {
    margin: 0;
    font-size: var(--text-overview-figure);
    font-weight: 600;
    font-variant-numeric: tabular-nums;
    letter-spacing: -0.03em;
    line-height: 1.05;
  }

  .sub {
    margin: 0;
    color: var(--color-muted);
    font-size: var(--text-body);
  }

  .dot {
    margin-inline: 6px;
    color: var(--color-faint);
  }

  .change {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    margin: 0;
    padding: 3px 10px 3px 8px;
    border-radius: var(--radius-capsule);
    background: var(--color-fill);
    color: var(--color-label-secondary);
    font-size: var(--text-label);
    font-variant-numeric: tabular-nums;
  }

  .site-actions {
    display: flex;
    gap: 8px;
  }

  .sites {
    padding: 14px 10px;
  }

  .stats {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 8px;
    padding: 16px;
  }

  .stats div {
    display: grid;
    gap: 2px;
  }

  .stat {
    font-size: var(--text-object-title);
    font-weight: 600;
    font-variant-numeric: tabular-nums;
  }

  .label {
    color: var(--color-muted);
    font-size: var(--text-caption);
  }

  .shut {
    display: grid;
    gap: 6px;
    scroll-margin-block-start: 12px;
  }

  h3 {
    margin: 0;
    font-size: var(--text-body);
    font-weight: 600;
  }

  .help {
    margin: 0 0 8px;
    color: var(--color-muted);
    font-size: var(--text-label);
  }

  .quiet {
    margin: 0;
    color: var(--color-muted);
    font-size: var(--text-label);
  }

  .local {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 5px;
    margin: 0;
    color: var(--color-faint);
    font-size: var(--text-caption);
  }

  @keyframes enter {
    from {
      opacity: 0;
      transform: translateY(4px);
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .site-head {
      animation: none;
    }
  }

  :global(:root[data-reduce-motion="true"]) .site-head {
    animation: none;
  }
</style>
