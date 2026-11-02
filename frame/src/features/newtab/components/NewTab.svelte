<script lang="ts">
  import { onMount, untrack, type Snippet } from "svelte";
  import { accountFoot, accountSession } from "$domain/account";
  import {
    CheckListIcon,
    Clock01Icon,
    Settings01Icon,
    Shield01Icon,
  } from "@hugeicons/core-free-icons";
  import * as m from "$shared/i18n/messages";
  import { commands } from "$shared/ipc/bindings";
  import { events } from "$shared/ipc/native-events";
  import { preferences } from "$domain/preferences";
  import { newTabImage, imageFailed } from "$session/newtab-background.svelte";
  import { taskCounts } from "$domain/resources";
  import { tabs } from "$domain/tabs";
  import Icon from "$shared/ui/Icon";
  import IconButton from "$shared/ui/IconButton";
  import { greetingFor } from "../lib/greeting";
  import { clockFace, clockFormat, dayKey, focusSpan, untilNextMinute } from "../lib/clock";
  import { readSystemClock, systemHour24 } from "../lib/system-clock.svelte";
  import { focus, localDay } from "$domain/time";
  import { groundPath, notchShape, notchWidth } from "../lib/ground";
  import { layout, type Box } from "../lib/layout";
  import { WORDMARK } from "$shared/lib/wordmark";
  import { reducedMotion } from "$shared/lib/motion";
  import {
    decode,
    FROSTS,
    placeSky,
    placeSkyInWindow,
    SCENES,
    scene as skyScene,
    seen,
    type Scene,
  } from "$shared/sky";
  import NewTabAccount from "./NewTabAccount.svelte";

  let {
    search,
    oncustomize,
    onaccount,
    ontasks,
    ontime,
  }: {
    /** The field that hangs in the notch. */
    search: Snippet;
    /** Opens this page's settings. */
    oncustomize?: () => void;
    /** Opens the account in Settings, or straight onto buying credits. */
    onaccount?: (intent: "account" | "credits") => void;
    /** Opens Tasks. */
    ontasks?: () => void;
    /** Opens Time. */
    ontime?: () => void;
  } = $props();

  let width = $state(0);
  let height = $state(0);
  let now = $state(new Date());
  let due = $state({ today: 0, overdue: 0 });
  let blocked = $state<number | null>(null);
  let focusedBefore = $state(0);

  let incognito = $derived(tabs.profile()?.kind === "incognito");
  const account = accountFoot({ private: () => incognito });
  // The account is in its own pane beside the page's settings, and the
  // settings keep the corner, so nothing moves when the account is known.
  let showAccount = $derived(!!onaccount && account.available);
  let signInWidth = $state(0);
  let profile = $derived(tabs.profile()?.id ?? null);
  let picture = $derived(newTabImage());
  let useSky = $derived(picture === null);
  let background = $derived.by(() => {
    const selected = picture;
    return selected
      ? import("./CustomBackground.svelte").catch(() => {
          imageFailed(selected.key);
          return null;
        })
      : null;
  });
  let showWordmark = $derived(preferences.value("ui.newtab-wordmark") !== "false");
  let showTrackers = $derived(preferences.value("ui.newtab-trackers") !== "false");
  let showGreeting = $derived(preferences.value("ui.newtab-greeting") === "true");
  let showClock = $derived(preferences.value("ui.newtab-clock") === "true");
  let showDue = $derived(
    !incognito && preferences.enabled("tasks") && preferences.value("ui.newtab-tasks") === "true",
  );
  let showFocus = $derived(
    preferences.enabled("time") && preferences.value("ui.newtab-focus") !== "false",
  );
  let today = $derived(dayKey(now));

  let firstName = $derived((tabs.profile()?.name ?? "").trim().split(/\s+/u)[0] ?? "");
  let greeting = $derived.by(() => {
    const words = greetingFor(now);
    const named = !incognito && preferences.value("ui.newtab-name") === "true" && firstName;
    return named ? m.ntp_greeting_named({ greeting: words, name: firstName }) : words;
  });
  let clockSetting = $derived(clockFormat(preferences.value("ui.newtab-clock-format")));
  let face = $derived(clockFace(now, clockSetting, undefined, systemHour24()));
  // The system's own setting is read when the page opens and when the window
  // comes back, which is where a change made in System Settings arrives.
  $effect(() => {
    if (!showClock || clockSetting !== "system") return;
    void readSystemClock();
    const reread = () => void readSystemClock();
    window.addEventListener("focus", reread);
    return () => window.removeEventListener("focus", reread);
  });
  let date = $derived(
    new Intl.DateTimeFormat(undefined, { weekday: "long", month: "long", day: "numeric" }).format(
      now,
    ),
  );
  const number = new Intl.NumberFormat();
  let running = $derived(focus.session());
  let focusMinutes = $derived(
    (focusedBefore + (running === null ? 0 : focus.focusedSeconds(running, now.getTime()))) / 60,
  );
  let focusGoalMinutes = $derived(Number(preferences.value("focus.goal")) || 120);
  let focusShare = $derived(
    focusGoalMinutes > 0 ? Math.min(1, Math.max(0, focusMinutes / focusGoalMinutes)) : 0,
  );

  // Private windows show no figures. Each requested figure closes up beside
  // its neighbours; hidden figures do not read their counters.
  let figures = $derived(
    incognito
      ? []
      : [
          ...(showTrackers ? ["trackers"] : []),
          ...(showFocus ? ["focus"] : []),
          ...(showDue ? ["due"] : []),
        ],
  );

  // Read when the page opens and with the minute's redraw while it shows, so
  // it keeps up with browsing in other tabs without a timer of its own.
  $effect(() => {
    const owner = profile;
    void now;
    if (!owner || incognito || !showTrackers) {
      blocked = null;
      return;
    }
    let live = true;
    void commands.blockerStats(owner).then(
      (result) => {
        if (live) blocked = result.status === "ok" ? result.data.today : null;
      },
      () => {},
    );
    return () => {
      live = false;
    };
  });
  // Focus finished earlier today; a running session adds to it live.
  $effect(() => {
    const owner = profile;
    const day = localDay(now);
    void running?.rounds;
    if (!owner || incognito || !showFocus) {
      focusedBefore = 0;
      return;
    }
    let live = true;
    void commands.timeCall(owner, { kind: "focus_days", from_day: day, days: 1 }).then(
      (result) => {
        if (live && result.kind === "focus_days") focusedBefore = result.days[0]?.seconds ?? 0;
      },
      () => {},
    );
    return () => {
      live = false;
    };
  });
  let controls = $derived([
    ...(showAccount ? [account.signedIn ? 32 : Math.max(32, signInWidth)] : []),
    ...(oncustomize ? [32] : []),
  ]);
  let page = $derived(
    layout(width, height, {
      controls,
      clock: showClock,
      wordmark: showWordmark,
    }),
  );
  const at = ({ x, y, w, h }: Box) => `left:${x}px;top:${y}px;width:${w}px;height:${h}px`;

  // Day over a light page, night over a dark one.
  let scene = $derived(skyScene());

  // The sky arrives once a session, as the first new tab opens, and is simply
  // there on every one after.
  const arriving = !seen(untrack(() => scene));
  let shown = $state<Scene | null>(seen(untrack(() => scene)) ? untrack(() => scene) : null);
  $effect(() => {
    if (!useSky) return;
    const wanted = scene;
    let live = true;
    // A sky that cannot be decoded leaves the page its own colour; it never
    // keeps the name and the controls hidden.
    const reveal = () => {
      if (live) shown = wanted;
    };
    void decode(wanted).then(reveal, reveal);
    return () => {
      live = false;
    };
  });

  // The picture is the page: painted with it, nothing drawn of its own. The
  // page is cut by the notch the field hangs in, so the window's own glass
  // reaches down into it around the field.
  let sky = $derived(placeSky(width, page.horizon));
  // The glass beside the page is lit from the picture's left edge, so it is
  // told where the picture lies in the window whenever either moves.
  let section = $state<HTMLElement>();
  $effect(() => {
    if (!useSky) return;
    const { y, h } = sky;
    const pageHeight = height;
    void width;
    if (!section || h <= 0) return;
    // Laid out rather than as drawn: the page may be mid-flight in a push.
    let left = 0;
    let top = 0;
    for (let at: HTMLElement | null = section; at; at = at.offsetParent as HTMLElement | null) {
      left += at.offsetLeft;
      top += at.offsetTop;
    }
    placeSkyInWindow({ left, top, height: pageHeight, y, h });
  });
  const id = $props.id();
  let ground = $derived(groundPath(width, height, page.notch));
  let lip = $derived(notchShape(width, height, page.notch));
  let fieldWidth = $derived(notchWidth(width, page.notch));
  let letters = $derived(
    `scale(${page.mark.w / WORDMARK.width}) ` +
      `translate(${-WORDMARK.x} ${-WORDMARK.y}) scale(${WORDMARK.scaleX} ${WORDMARK.scaleY})`,
  );
  // One pass of light across the name as the first sky arrives, then
  // stillness: driven by the document, nothing left running once it passes.
  const glint = arriving && !reducedMotion();

  // What is due today, overdue included, kept current while the page shows it:
  // read once, then again whenever a task in this profile changes.
  $effect(() => {
    const owner = profile;
    const day = today;
    if (!owner || !showDue) {
      due = { today: 0, overdue: 0 };
      return;
    }
    let live = true;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const read = () =>
      void taskCounts(owner, day).then(
        (counts) => {
          if (live && counts) due = { today: counts.today, overdue: counts.overdue };
        },
        () => {},
      );
    read();
    const listening = events.resourceChanged.listen(({ payload }) => {
      if (payload.profile !== owner || payload.kind !== "task") return;
      clearTimeout(timer);
      timer = setTimeout(read, 250);
    });
    return () => {
      live = false;
      clearTimeout(timer);
      void listening.then((stop) => stop());
    };
  });

  onMount(() => {
    void accountSession.retain();
    return () => accountSession.release();
  });

  onMount(() => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    // One redraw per minute, on the turn, and none while the page is hidden.
    function update() {
      now = new Date();
      clearTimeout(timer);
      if (document.visibilityState === "visible")
        timer = setTimeout(update, untilNextMinute(Date.now()));
    }
    function visibility() {
      clearTimeout(timer);
      if (document.visibilityState === "visible") update();
    }
    update();
    document.addEventListener("visibilitychange", visibility);
    return () => {
      clearTimeout(timer);
      document.removeEventListener("visibilitychange", visibility);
    };
  });
</script>

<!--
  Drawn by the chrome, so a new tab costs no WebView and shows no paint flash.
  The page is a window onto the Zephium sky: solid above, dissolving through
  a halftone screen into the page below the horizon. The window's own glass
  reaches down into it through the notch the field hangs in. The dots are
  baked into the picture, so the page only shows it.
-->
<section
  bind:this={section}
  class="newtab"
  class:arriving={useSky && arriving}
  class:entering={useSky && !arriving}
  class:custom={!useSky}
  data-scene={scene}
  data-ready={!useSky || shown === scene}
  aria-label={m.new_tab()}
  style:--sky-frost={useSky ? `url("${FROSTS[scene]}")` : undefined}
  style:--sky-frost-size={`${sky.w}px ${sky.h}px`}
  style:--sky-frost-x={`${sky.x - (width - fieldWidth) / 2}px`}
  style:--sky-frost-y={`${sky.y - page.notch.height}px`}
  bind:clientWidth={width}
  bind:clientHeight={height}
>
  {#if ground}<div class="page" aria-hidden="true" style:clip-path={`path(evenodd, "${ground}")`}>
      {#if picture && width > 0 && height > 0}
        {#key picture.key}
          {@const selected = picture}
          {#await background then loaded}
            {#if loaded}<loaded.default
                profile={picture.profile}
                digest={picture.digest}
                {width}
                {height}
                dim={preferences.value("ui.newtab-dim") === "true"}
                onfailure={() => {
                  imageFailed(selected.key);
                }}
              />{/if}
          {/await}
        {/key}
      {:else if useSky}<div
          class="sky"
          style:background-image={`url("${SCENES[scene]}")`}
          style:background-size={`${sky.w}px ${sky.h}px`}
          style:background-position={`${sky.x}px ${sky.y}px`}
        ></div>{/if}
    </div>{/if}

  <!-- Light caught along the edge where the page meets the window, fading
       away from it: a strip as tall as the notch, so it is drawn once. -->
  {#if useSky}<svg class="rim" aria-hidden="true" style:height={`${page.notch.height + 1}px`}>
      <defs>
        <linearGradient id={`${id}-lip`} x1="0" y1="1" x2="0" y2="0">
          <stop offset="0" class="lip" />
          <stop offset="0.6" class="lip" stop-opacity="0" />
        </linearGradient>
      </defs>
      <path d={lip} stroke={`url(#${id}-lip)`} />
    </svg>
  {/if}

  <!-- The name, drawn only as large as itself. -->
  {#if showWordmark}<svg class="mark" role="img" aria-label="Zephium" style={at(page.mark)}>
      <defs>
        <path id={`${id}-letters`} d={WORDMARK.d} transform={letters} />
        {#if glint && useSky}<linearGradient
            id={`${id}-glint`}
            gradientUnits="userSpaceOnUse"
            x1={-page.mark.h * 3}
            x2="0"
            y1="0"
            y2={page.mark.h}
          >
            <stop offset="0" class="glint" stop-opacity="0" />
            <stop offset="0.5" class="glint" />
            <stop offset="1" class="glint" stop-opacity="0" />
            <animateTransform
              attributeName="gradientTransform"
              type="translate"
              from="0 0"
              to={`${page.mark.w + page.mark.h * 3} 0`}
              begin="0.5s"
              dur="1.3s"
              calcMode="spline"
              keyTimes="0;1"
              keySplines="0.3 0 0.2 1"
              fill="freeze"
            />
          </linearGradient>{/if}
      </defs>
      <use class="name" href={`#${id}-letters`} />
      {#if glint && useSky}<use href={`#${id}-letters`} fill={`url(#${id}-glint)`} />{/if}
    </svg>
  {/if}

  <!-- The band the field hangs in is the window's title bar: beside the
       field, it moves the window. -->
  <div
    class="drag"
    data-tauri-drag-region
    aria-hidden="true"
    style:height={`${page.notch.height}px`}
  ></div>

  <div class="field" style:width={`${fieldWidth}px`} style:height={`${page.notch.height}px`}>
    {@render search()}
  </div>

  {#if showGreeting}
    <p class="greeting" style:bottom={`${height - page.mark.y + 18}px`}>
      {greeting}
    </p>
  {/if}

  {#if page.when !== null}
    <p class="when" style:top={`${page.when}px`}>
      <time class="time" datetime={now.toISOString()}
        >{#if face.period !== null && face.periodLeading}<span class="period"
            >{`${face.period} `}</span
          >{/if}{face.time}{#if face.period !== null && !face.periodLeading}<span class="period"
            >{` ${face.period}`}</span
          >{/if}</time
      ><span class="date">{date}</span>
    </p>
  {/if}

  <!-- Each control a disc of light on the sky, level with the field. -->
  {#if showAccount && onaccount && page.controls[0]}
    <div class="control" style={at(page.controls[0])}>
      <NewTabAccount {account} {onaccount} bind:signInWidth />
    </div>
  {/if}
  {#if oncustomize && page.controls.at(-1)}
    <div class="control" style={at(page.controls.at(-1)!)}>
      <IconButton
        icon={Settings01Icon}
        label={m.settings_customize_newtab()}
        shape="circle"
        size={16}
        buttonSize={32}
        onclick={oncustomize}
      />
    </div>
  {/if}

  <!-- A private window has no figures; what it promises stands in their place. -->
  {#if incognito}
    <p class="note">{m.ntp_private()}</p>
  {:else if figures.length}
    <!-- The day in figures, along the foot of the page; they step aside
         while a search is open. -->
    <div class="figures">
      {#each figures as figure (figure)}
        {#if figure === "trackers"}
          <div class="figure">
            <Icon icon={Shield01Icon} size={14} /><span class="name"
              >{m.ntp_trackers_blocked()}</span
            ><span class="value">{number.format(blocked ?? 0)}</span>
          </div>
        {:else if figure === "focus"}
          <button type="button" class="figure" onclick={ontime}>
            <Icon icon={Clock01Icon} size={14} /><span class="name">{m.ntp_focused()}</span><span
              class="value">{focusSpan(focusMinutes)}</span
            ><span
              class="meter"
              role="meter"
              aria-label={m.ntp_focus_goal({ goal: focusSpan(focusGoalMinutes) })}
              aria-valuemin={0}
              aria-valuemax={focusGoalMinutes}
              aria-valuenow={Math.min(focusMinutes, focusGoalMinutes)}
              ><span class="fill" style:transform={`scaleX(${focusShare})`}></span></span
            >
          </button>
        {:else}
          <button type="button" class="figure" onclick={ontasks}>
            <Icon icon={CheckListIcon} size={14} /><span class="name">{m.ntp_due_today()}</span
            ><span class="value">{due.today}</span><span class="foot" class:late={due.overdue > 0}
              >{due.overdue > 0
                ? m.ntp_overdue({ count: due.overdue })
                : m.ntp_nothing_overdue()}</span
            >
          </button>
        {/if}
      {/each}
    </div>
  {/if}
</section>

<style>
  /* Whatever the page leaves open is the window: the chrome's own colour on
     an opaque window, and its material where it has one. Below the horizon
     the page is the colour the sky dissolves into. Everything is placed in
     the page's physical pixels, which do not mirror in a right-to-left page. */
  .newtab {
    --sky-paper: var(--sky-day-paper);
    --sky-shadow: var(--sky-day-shadow);
    --sky-lip: var(--sky-day-lip);
    --disc-fill: var(--sky-day-disc-fill);
    --disc-rim: var(--sky-day-disc-rim);
    --sky-card: var(--sky-day-card);
    --sky-card-rim: var(--sky-day-card-rim);
    --sky-card-edge: var(--sky-day-card-edge);
    --sky-edge: var(--sky-day-edge);
    --sky-edge-half: var(--sky-day-edge-half);
    --sky-edge-faint: var(--sky-day-edge-faint);

    position: relative;
    block-size: 100%;
    overflow: hidden;
    background: var(--color-chrome);
    isolation: isolate;
    container: newtab / inline-size;
  }

  .newtab[data-scene="night"] {
    --sky-paper: var(--sky-night-paper);
    --sky-shadow: var(--sky-night-shadow);
    --sky-lip: var(--sky-night-lip);
    --disc-fill: var(--sky-night-disc-fill);
    --disc-rim: var(--sky-night-disc-rim);
    --sky-card: var(--sky-night-card);
    --sky-card-rim: var(--sky-night-card-rim);
    --sky-card-edge: var(--sky-night-card-edge);
    --sky-edge: var(--sky-night-edge);
    --sky-edge-half: var(--sky-night-edge-half);
    --sky-edge-faint: var(--sky-night-edge-faint);
  }

  :global(:root:not([data-material="none"])[data-material]) .newtab {
    background: transparent;
  }

  .newtab.custom {
    --sky-card: var(--color-field);
    --sky-card-rim: var(--color-border);
    --sky-card-edge: var(--color-border);
    --disc-fill: var(--color-surface);
    --sky-shadow: var(--newtab-image-dim);
    --disc-rim: var(--color-border);
  }

  .page,
  .sky {
    position: absolute;
    inset: 0;
  }

  .page {
    background: var(--sky-paper);
  }

  /* The picture eases off at the page's sides rather than ending at them,
     so a bright cloud sinks into the edge instead of meeting whatever the
     window's glass is showing beside it. */
  .newtab:not(.custom) .page::after {
    position: absolute;
    inset: 0;
    background: linear-gradient(
      to right,
      var(--sky-edge) 0,
      var(--sky-edge-half) 2.5%,
      var(--sky-edge-faint) 5.5%,
      transparent 9%,
      transparent 91%,
      var(--sky-edge-faint) 94.5%,
      var(--sky-edge-half) 97.5%,
      var(--sky-edge) 100%
    );
    content: "";
    pointer-events: none;
  }

  .sky {
    background-repeat: no-repeat;
  }

  .rim {
    position: absolute;
    inset-block-start: 0;
    inset-inline: 0;
    inline-size: 100%;
    pointer-events: none;
  }

  /* Light along the notch's foot: white by day, a cool moonlit tone at
     night, where white would read as a drawn border. */
  .rim path {
    fill: none;
    stroke-width: 1;
  }

  .lip {
    stop-color: var(--sky-lip);
  }

  .mark {
    position: absolute;
    overflow: visible;
    pointer-events: none;
  }

  .name {
    fill: var(--sky-name);
  }

  .glint {
    stop-color: var(--sky-glint);
  }

  .drag {
    position: absolute;
    z-index: 1;
    inset-block-start: 0;
    inset-inline: 0;
  }

  /* The field fills the notch: the window's glass is its material. */
  .field {
    position: absolute;
    z-index: 2;
    inset-block-start: 0;
    inset-inline: 0;
    margin-inline: auto;
  }

  .greeting,
  .when {
    position: absolute;
    inset-inline: 0;
    margin: 0 auto;
    color: var(--sky-ink);
    text-align: center;
    text-shadow: 0 1px 18px var(--sky-shadow);
    pointer-events: none;
  }

  .greeting {
    font-size: 11.5px;
    font-weight: 600;
    line-height: 16px;
    letter-spacing: 0.18em;
    text-transform: uppercase;
    opacity: 0.86;
  }

  /* One line under the name: the time set firm, the date beside it a step
     quieter, a point of light between them. */
  .when {
    display: flex;
    align-items: baseline;
    justify-content: center;
    gap: 8px;
    font-size: 16px;
    line-height: 20px;
  }

  .time {
    font-weight: 600;
    font-variant-numeric: tabular-nums;
    letter-spacing: 0.01em;
  }

  .period {
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.08em;
    opacity: 0.86;
  }

  .date {
    font-weight: 400;
    opacity: 0.86;
  }

  .date::before {
    content: "";
    display: inline-block;
    inline-size: 3px;
    block-size: 3px;
    margin-inline-end: 8px;
    border-radius: var(--radius-capsule);
    background: currentcolor;
    vertical-align: middle;
    opacity: 0.6;
  }

  /* A disc of light on the sky: the sky behind it is still, so it needs no
     blur of its own. */
  .control {
    position: absolute;
    z-index: 1;
    display: grid;
    place-items: center;
    border-radius: var(--radius-capsule);
    background: var(--disc-fill);
    box-shadow:
      inset 0 0 0 0.5px var(--disc-rim),
      inset 0 1px 0 var(--disc-rim);
    color: var(--sky-ink);
  }

  /* White on the sky for the buttons alone: the account's panel opens from
     inside the disc and keeps the theme's own ink. */
  .control :global(:is(.ntp-account, .sign-in, .icon-button)) {
    --color-text: var(--sky-ink);
    --color-muted: var(--sky-ink);
    --color-label-secondary: var(--sky-ink);
  }

  .note {
    position: absolute;
    inset-inline: 0;
    inset-block-end: 28px;
    margin: 0 auto;
    padding-inline: 24px;
    color: var(--color-faint);
    font-size: var(--text-label);
    line-height: 1.5;
    text-align: center;
  }

  /* The day's figures: quiet capsules on the page, each read as one line. */
  .figures {
    position: absolute;
    inset-inline: 0;
    inset-block-end: 24px;
    display: flex;
    flex-wrap: wrap;
    justify-content: center;
    gap: 8px;
    padding-inline: 24px;
    transition:
      opacity var(--motion-base) var(--ease-out),
      translate var(--motion-base) var(--ease-out);
  }

  /* Open results take the sky to themselves: the name and the time step
     back under them rather than showing round them, and return as soon as
     the results close, whatever is left in the field. Each is only as large as
     itself, so the fade composites a few small layers and repaints nothing. */
  .mark,
  .greeting,
  .when {
    transition: opacity var(--motion-base) var(--ease-out);
  }

  .newtab:has(:global(.dock[data-open="true"])) :is(.mark, .greeting, .when) {
    opacity: 0;
  }

  .figure {
    display: flex;
    align-items: center;
    gap: 7px;
    block-size: 34px;
    padding: 0 14px 0 12px;
    border: 0;
    border-radius: var(--radius-capsule);
    background: color-mix(in srgb, var(--color-text) 5%, transparent);
    color: var(--color-muted);
    font: inherit;
    font-size: 12.5px;
    font-weight: 500;
    white-space: nowrap;
    animation: arrive var(--motion-page) var(--ease-emphasized) 80ms backwards;
  }

  .figure :global(svg) {
    flex: none;
    color: var(--color-faint);
  }

  button.figure {
    cursor: default;
    outline: none;
    transition:
      background-color var(--motion-instant) var(--ease-smooth),
      scale var(--motion-slow) var(--ease-smooth);
  }

  button.figure:hover {
    background: color-mix(in srgb, var(--color-text) 9%, transparent);
  }

  button.figure:active {
    scale: 0.97;
    transition-duration: var(--motion-fast);
  }

  button.figure:focus-visible {
    outline: 2px solid var(--color-ring);
    outline-offset: 2px;
  }

  .value {
    color: var(--color-text);
    font-weight: 600;
    font-variant-numeric: tabular-nums;
  }

  .foot {
    color: var(--color-faint);
  }

  .foot.late {
    color: var(--color-danger);
  }

  /* The day's progress toward the aim. */
  .meter {
    position: relative;
    flex: none;
    inline-size: 28px;
    block-size: 3px;
    margin-inline-start: 2px;
    overflow: hidden;
    border-radius: var(--radius-capsule);
    background: var(--color-fill-strong);
  }

  .fill {
    position: absolute;
    inset: 0;
    border-radius: inherit;
    background: var(--color-text);
    transform-origin: left center;
  }

  .custom .control {
    --sky-ink: var(--color-text);
  }

  .custom .figure {
    background: var(--color-surface);
  }

  /* A narrow pane keeps each figure to its icon and value on one row; the
     rest is still read out. */
  @container newtab (inline-size < 720px) {
    .figure .name,
    .figure .foot {
      position: absolute;
      inline-size: 1px;
      block-size: 1px;
      overflow: hidden;
      clip-path: inset(50%);
    }
  }

  /* The first new tab of a session waits for its sky, then the sky and
     what stands on it come up together. */
  .arriving :is(.sky, .mark, .greeting, .when, .control) {
    transition: opacity 640ms var(--ease-out);
  }

  .arriving:not([data-ready="true"]) :is(.sky, .mark, .greeting, .when, .control) {
    opacity: 0;
  }

  /* Every new tab after the first comes up out of its own horizon: the
     picture settles in as the window turns to the sky, and the name rises
     out of the dots into focus, with the time a beat behind it. */
  .entering .page {
    animation: settle 320ms var(--ease-out) backwards;
  }

  .entering .mark {
    animation: rise 720ms var(--ease-emphasized) 40ms backwards;
  }

  .entering :is(.greeting, .when) {
    animation: rise-after 640ms var(--ease-emphasized) 120ms backwards;
  }

  @keyframes settle {
    from {
      opacity: 0;
    }
  }

  @keyframes rise {
    from {
      opacity: 0;
      filter: blur(8px);
      translate: 0 18px;
    }
  }

  @keyframes rise-after {
    from {
      opacity: 0;
      translate: 0 10px;
    }
  }

  @keyframes arrive {
    from {
      opacity: 0;
    }
  }

  @media (forced-colors: active) {
    .sky {
      display: none;
    }

    .name {
      fill: CanvasText;
    }

    .greeting,
    .when {
      color: CanvasText;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .figure,
    .entering :is(.page, .mark, .greeting, .when) {
      animation: none;
    }

    .arriving :is(.sky, .mark, .greeting, .when, .control),
    .figures {
      transition: none;
    }
  }

  :global(:root[data-reduce-motion="true"])
    :is(.figure, .entering .page, .entering .mark, .entering .greeting, .entering .when) {
    animation: none;
  }

  :global(:root[data-reduce-motion="true"])
    :is(.arriving .sky, .arriving .mark, .arriving .control, .figures) {
    transition: none;
  }
</style>
