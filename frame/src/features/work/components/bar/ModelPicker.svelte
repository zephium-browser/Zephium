<script lang="ts">
  import { Popover } from "bits-ui";
  import { onDestroy, onMount, tick, untrack } from "svelte";
  import { accountSession, planName } from "$domain/account";
  import type { WorkModelEntry, WorkModelProvider, WorkModelReasoning } from "$shared/ipc/bindings";
  import { commands } from "$shared/ipc/bindings";
  import {
    ModelsSession,
    effortLevels,
    effortName,
    entryOf,
    favourites,
    keyState,
    keyedProviders,
    locked,
    pickerGroups,
    providerName,
    requiredPlan,
    searchEntries,
    shortName,
    toggleFavourite,
    usable,
  } from "$domain/ai";
  import Icon from "$shared/ui/Icon";
  import ProviderMark from "$shared/ui/ProviderMark";
  import { menuMaterialFor } from "$shared/ui/Menu/material";
  import {
    Cancel01Icon,
    Key01Icon,
    Search01Icon,
    Settings02Icon,
    StarIcon,
    Tick02Icon,
  } from "../../lib/icons";
  import * as m from "$shared/i18n/messages";
  import { nearestEffort } from "./ModelEffort.svelte";
  import ModelAsk from "./ModelAsk.svelte";
  import ModelCard, { type CardSubject } from "./ModelCard.svelte";
  import ModelRail, { type Seat, type View } from "./ModelRail.svelte";
  import { tiers } from "./model-tier";
  import "$shared/ui/Menu/popover.css";
  import "$shared/ui/Shine/shine.css";

  let {
    profile,
    role = "lead",
    onsettings = () => void commands.runCommand("settings.ai"),
  }: {
    profile: string;
    /** Which of the person's models it chooses: Work's lead, or the sidebar assistant's page model. */
    role?: "lead" | "page";
    /** Opens Settings → Models, where keys are added. */
    onsettings?: () => void;
  } = $props();

  const uid = $props.id();

  let session = $state.raw<ModelsSession | null>(null);
  $effect(() => {
    const owner = new ModelsSession(profile);
    session = owner;
    void owner.start();
    return () => owner.dispose();
  });

  onMount(() => {
    void accountSession.retain();
    return () => accountSession.release();
  });
  const account = $derived(accountSession.account());
  /** Hosted models this server offers to someone not yet signed in. */
  const hostedOffer = $derived(
    !!account?.available && !account.signed_in && account.hosted_models === true,
  );

  let open = $state(false);
  let trigger = $state<HTMLElement | null>(null);
  let material = $state<"opaque" | undefined>();
  let list = $state<HTMLElement>();
  let main = $state<HTMLElement>();
  let field = $state<HTMLInputElement>();

  const models = $derived(session?.models ?? null);
  const current = $derived(entryOf(models, models?.effective[role]));
  const chosen = $derived(models?.effective[role] ?? null);
  const groups = $derived(pickerGroups(models, role, session?.endpoint ?? []));
  const tierOf = $derived(tiers(groups.ready));

  type Item = { id: string; entry: WorkModelEntry };
  type Section = { key: string; label: string; provider: WorkModelProvider; items: Item[] };

  const nameOf = (provider: WorkModelProvider) =>
    provider === "cloud"
      ? m.ai_cloud_title()
      : provider === "compatible"
        ? m.ai_custom_name()
        : providerName(provider);

  /** Every provider Settings offers: Zephium's own first, then the ready ones, then each with the one step that readies it. */
  const seats = $derived.by<Seat[]>(() => {
    const own: Seat[] = [];
    const ready: Seat[] = [];
    const rest: Seat[] = [];
    for (const provider of ["cloud", ...keyedProviders] as const) {
      if (provider === "cloud") {
        if (usable(models, provider) || account?.signed_in)
          own.push({ provider, standing: "ready" });
        else if (account?.available && account.hosted_models === true)
          own.push({ provider, standing: "sign_in" });
      } else if (usable(models, provider)) ready.push({ provider, standing: "ready" });
      else
        rest.push({
          provider,
          standing:
            provider === "compatible" &&
            !models?.providers.find((row) => row.provider === provider)?.base
              ? "server"
              : keyState(models, provider) === "invalid"
                ? "refused"
                : "key",
        });
    }
    return [...own, ...ready, ...rest];
  });

  const starred = $derived(new Set(favourites()));
  const hasStars = $derived(
    groups.ready.some((group) => group.entries.some((e) => starred.has(e.id))),
  );

  /** The sheet is as tall as its longest list needs, so it holds still as providers are chosen and searched. */
  const sheetHeight = $derived.by(() => {
    const rows = Math.max(0, ...groups.ready.map((group) => group.entries.length));
    // The rail's own height: its padding, a seat and a gap for each, and the hairline after the leading seats.
    const rail = 26 + seats.length * 34 + (seats[0]?.provider === "cloud" ? 8 : 0);
    return Math.min(520, Math.max(rail, 148 + 34 * rows));
  });

  let picked = $state<View | null>(null);
  const shown = $derived.by<View | null>(() => {
    if (picked === "favourites" && hasStars) return "favourites";
    return (
      seats.find((row) => row.provider === picked)?.provider ??
      seats.find((row) => row.provider === current?.model.provider)?.provider ??
      seats[0]?.provider ??
      null
    );
  });
  const seat = $derived(seats.find((row) => row.provider === shown) ?? null);

  let searchOpen = $state(false);
  let query = $state("");
  const searching = $derived(query.trim().length > 0);
  /** Favourites and search span providers, so their rows are grouped under each one's name. */
  const spanning = $derived(searching || shown === "favourites");
  const sections = $derived.by<Section[]>(() => {
    const matched = searching
      ? new Set(searchEntries(groups.ready, query).map((entry) => entry.id))
      : null;
    return groups.ready
      .filter((group) => spanning || group.provider === shown)
      .map((group) => ({
        key: group.provider,
        label: nameOf(group.provider),
        provider: group.provider,
        items: group.entries
          .filter((entry) =>
            matched ? matched.has(entry.id) : shown !== "favourites" || starred.has(entry.id),
          )
          .map((entry) => ({ id: entry.id, entry })),
      }))
      .filter((section) => section.items.length);
  });
  const hits = $derived(searching ? new Set(sections.map((section) => section.provider)) : null);
  const items = $derived(sections.flatMap((section) => section.items));
  const slot = (id: string) => `${uid}-${items.findIndex((item) => item.id === id)}`;

  /** The row the pointer or the arrows are on, which Enter chooses. */
  let active = $state<string | null>(null);
  const home = $derived(items.find((item) => item.id === chosen)?.id ?? items[0]?.id ?? null);
  const focus = $derived(items.find((item) => item.id === active) ?? null);

  /** The effort the person last set while the menu is open, ahead of Rust's answer. */
  let intent = $state<WorkModelReasoning | null>(null);
  const levels = $derived(effortLevels(models, chosen));
  const effort = $derived<WorkModelReasoning | null>(
    levels.length ? (intent ? nearestEffort(levels, intent) : (models?.effort ?? null)) : null,
  );

  const card = $derived.by<CardSubject | null>(() => {
    if (!current) return null;
    return { entry: current, tier: tierOf[current.id] ?? null, levels, effort };
  });

  /**
   * Whether a run could start: the lead needs a model with a working key. Asked
   * again whenever the models change, so a key added in Settings lands here.
   */
  let leadMissing = $state(false);
  /** No chosen model can search and no OpenAI key stands in: runs work without the web. */
  let searchOff = $state(false);
  $effect(() => {
    void models;
    if (role !== "lead") return;
    let live = true;
    void Promise.resolve()
      .then(() => commands.workModelsReady(profile))
      .then((readiness) => {
        if (!live) return;
        leadMissing = !readiness.ready && readiness.missing_roles.includes("lead");
        searchOff = readiness.ready && readiness.search === "unavailable";
      })
      .catch(() => {
        if (live) {
          leadMissing = false;
          searchOff = false;
        }
      });
    return () => (live = false);
  });
  /** No model can run yet: the one thing to do is add a key, so the control is that, not a menu. */
  const keyless = $derived(!!models && leadMissing);

  /** The card shows the effort control until it is put away: by Escape, a click elsewhere, or a rest. */
  let effortOpen = $state(false);
  let pill = $state<HTMLElement>();
  let fold: ReturnType<typeof setTimeout> | undefined;
  /** The thumb is in the person's hand. */
  let held = false;
  /** The level was changed since the control opened. */
  let changed = false;
  let stirredAt = 0;
  /** The menu closed on a choice or Escape: the person is mid-sentence, so the words get the focus back. */
  let toWords = false;
  const IDLE = 3000;
  const SETTLED = 1600;

  function putAway() {
    clearTimeout(fold);
    changed = false;
    if (!effortOpen) return;
    effortOpen = false;
    if (main?.contains(document.activeElement)) void tick().then(() => pill?.focus());
  }

  /** The control turns back to the model once it has been left alone a while; never under a hand. */
  function rest() {
    clearTimeout(fold);
    if (effortOpen && !held) fold = setTimeout(putAway, changed ? SETTLED : IDLE);
  }

  function toggleEffort() {
    if (effortOpen) {
      putAway();
      return;
    }
    effortOpen = true;
    rest();
  }

  /** Any use of the control keeps it open a while longer. */
  function stirred() {
    const now = performance.now();
    if (now - stirredAt < 300) return;
    stirredAt = now;
    rest();
  }

  function chose(level: WorkModelReasoning) {
    void setEffort(level);
    changed = true;
    rest();
  }

  onDestroy(() => clearTimeout(fold));

  function reset() {
    query = "";
    searchOpen = false;
    active = null;
    intent = null;
    picked = null;
    clearTimeout(fold);
    effortOpen = false;
    changed = false;
    held = false;
  }

  /** The person was writing: whatever closes the menu hands the focus back to the words, never to nothing. */
  function toField() {
    const field = trigger?.closest(".surface")?.querySelector("textarea");
    const home = () => {
      const at = document.activeElement;
      if (
        at &&
        at !== document.body &&
        !trigger?.contains(at) &&
        !main?.closest(".model-menu")?.contains(at)
      )
        return;
      field?.focus({ preventScroll: true });
    };
    home();
    // WebKit drops the focus once more when the press that closed the menu lands on the chip.
    setTimeout(home, 0);
  }

  // The menu hangs from the whole sheet, which the popover counts as its own trigger: a press
  // anywhere else on the sheet is the person going back to their words, so it closes here.
  $effect(() => {
    if (!open) return;
    const sheet = trigger?.closest<HTMLElement>(".surface");
    if (!sheet) return;
    const away = (event: PointerEvent) => {
      if (!(event.target instanceof Node) || trigger?.contains(event.target)) return;
      close();
    };
    sheet.addEventListener("pointerdown", away, true);
    return () => sheet.removeEventListener("pointerdown", away, true);
  });

  function opened(next: boolean) {
    open = next;
    if (!next) {
      toField();
      reset();
      return;
    }
    material = menuMaterialFor(trigger);
    // The person's own server says which models it serves; the rest is the curated list.
    if (usable(models, "compatible")) void session?.listEndpoint();
    active = home;
  }

  function close() {
    toWords = true;
    toField();
    open = false;
    reset();
  }

  /** The menu stays while the choice lands, so the card and the trigger change before it closes. */
  async function choose(entry: WorkModelEntry) {
    if (locked(models, entry.id)) return upgrade();
    if (session?.busy) return;
    if (entry.id !== chosen && !(await session?.choose(role, entry.id))) return;
    close();
  }

  /** Each change goes to Rust in turn; a newer one waits for the last to land. */
  let wanted: WorkModelReasoning | null = null;
  let sending = false;
  async function setEffort(level: WorkModelReasoning) {
    intent = level;
    wanted = level;
    if (sending) return;
    sending = true;
    try {
      while (session && wanted && wanted !== models?.effort) {
        const asked: WorkModelReasoning = wanted;
        if (!(await session.setEffort(asked))) {
          if (session.busy) {
            await new Promise((done) => setTimeout(done, 60));
            continue;
          }
          wanted = null;
          intent = null;
          break;
        }
        if (wanted === asked) wanted = null;
      }
    } finally {
      sending = false;
    }
  }

  /** A model the plan does not include opens Account, where the plans are. */
  function upgrade() {
    open = false;
    reset();
    void commands.runCommand("settings.account");
  }

  function settings() {
    open = false;
    reset();
    onsettings();
  }

  /** The step a provider waits on: signing in for the hosted models, otherwise Settings. */
  function step(row: Seat | null) {
    if (row?.standing === "sign_in") {
      open = false;
      reset();
      void commands.runCommand("settings.account");
    } else settings();
  }

  async function pick(view: View | null) {
    picked = view;
    query = "";
    searchOpen = false;
    await tick();
    active = items.find((item) => item.id === chosen)?.id ?? items[0]?.id ?? null;
    reveal(active);
    const at = document.activeElement;
    if (!at || at === document.body) (list ?? main)?.focus({ preventScroll: true });
  }

  /** The favourites view and back to the provider that was open. */
  let before: View | null = null;
  async function toggleFavourites() {
    if (shown === "favourites") {
      await pick(before);
      return;
    }
    before = shown;
    await pick("favourites");
  }

  function reveal(id: string | null) {
    if (!id) return;
    list?.querySelector(`[data-id="${CSS.escape(id)}"]`)?.scrollIntoView({ block: "nearest" });
  }

  /** The arrows, not the pointer, moved to the row: its star shows as the pointer's would. */
  let walked = $state(false);
  function walk(to: number) {
    const next = items[Math.max(0, Math.min(items.length - 1, to))];
    if (!next) return;
    walked = true;
    active = next.id;
    reveal(next.id);
  }

  function retyped() {
    active = items[0]?.id ?? null;
    void tick().then(() => reveal(active));
  }

  /** The search field slides in from its icon; typing anywhere in the menu opens it too. */
  async function openSearch(text = "") {
    searchOpen = true;
    if (text) query += text;
    await tick();
    field?.focus({ preventScroll: true });
    retyped();
  }

  function closeSearch() {
    searchOpen = false;
    query = "";
    active = home;
    (list ?? main)?.focus({ preventScroll: true });
  }

  function keys(event: KeyboardEvent) {
    if (event.defaultPrevented || event.isComposing) return;
    const target = event.target as HTMLElement;
    const inField = target === field;
    const onButton = target instanceof HTMLButtonElement;
    if (
      !inField &&
      !effortOpen &&
      event.key.length === 1 &&
      event.key !== " " &&
      !event.metaKey &&
      !event.ctrlKey &&
      !event.altKey
    ) {
      event.preventDefault();
      void openSearch(event.key);
      return;
    }
    if (onButton && (event.key === "Enter" || event.key === " ")) return;
    if (target.getAttribute("role") === "tab" && event.key.startsWith("Arrow")) return;
    if (target.getAttribute("role") === "slider") return;
    const at = items.findIndex((item) => item.id === active);
    switch (event.key) {
      case "ArrowDown":
      case "ArrowUp":
        event.preventDefault();
        walk(at === -1 ? 0 : at + (event.key === "ArrowDown" ? 1 : -1));
        return;
      case "Home":
      case "End":
        if (query) return;
        event.preventDefault();
        walk(event.key === "Home" ? 0 : items.length - 1);
        return;
      case " ":
        if (inField || !focus) return;
        event.preventDefault();
        toggleFavourite(focus.id);
        return;
      case "Enter": {
        const item = focus ?? items[0];
        event.preventDefault();
        if (item) void choose(item.entry);
        else if (!searching && seat && seat.standing !== "ready") step(seat);
        return;
      }
    }
  }

  function escape(event: KeyboardEvent) {
    if (effortOpen) {
      event.preventDefault();
      putAway();
      return;
    }
    if (searchOpen) {
      event.preventDefault();
      closeSearch();
      return;
    }
    toWords = true;
  }

  function returned(event: Event) {
    // Positioning can replace the focus scope while the menu is still open.
    if (open) {
      event.preventDefault();
      return;
    }
    if (!toWords) {
      const field = trigger?.closest(".surface")?.querySelector("textarea");
      if (field && document.activeElement === field) event.preventDefault();
      return;
    }
    toWords = false;
    event.preventDefault();
    trigger?.closest(".surface")?.querySelector("textarea")?.focus();
  }

  function pointed(event: PointerEvent) {
    walked = false;
    const id = (event.target as Element).closest<HTMLElement>("[data-id]")?.dataset.id;
    if (id && id !== active) active = id;
  }

  function clicked(event: MouseEvent) {
    const target = event.target as Element;
    const id = target.closest<HTMLElement>("[data-id]")?.dataset.id;
    if (!id) return;
    if (target.closest(".star")) {
      toggleFavourite(id);
      return;
    }
    const item = items.find((row) => row.id === id);
    if (item) void choose(item.entry);
  }

  /** One plate glides under the row the pointer or the arrows are on, rather than each row lighting on its own. */
  let plate = $state<{ y: number; h: number; glide: boolean } | null>(null);
  $effect(() => {
    void items;
    const id = active;
    const host = list;
    const at = id ? host?.querySelector<HTMLElement>(`[data-id="${CSS.escape(id)}"]`) : null;
    const before = untrack(() => plate);
    plate = at ? { y: at.offsetTop, h: at.offsetHeight, glide: !!before } : null;
  });

  const triggerLabel = $derived(
    current
      ? effort
        ? m.model_trigger_effort({ model: current.display_name, effort: effortName(effort) })
        : m.work_model_trigger({ model: current.display_name })
      : m.work_model_label(),
  );
  const title = $derived(
    shown === "favourites" ? m.model_favourites() : shown ? nameOf(shown) : m.work_model_label(),
  );
</script>

{#if keyless}
  <button type="button" class="model-trigger add-key ui-shine ui-shine-press" onclick={settings}>
    <span class="trigger-mark" aria-hidden="true"><Icon icon={Key01Icon} size={14} /></span>
    <span class="trigger-name"
      >{hostedOffer ? m.work_model_start() : m.work_model_add_model_key()}</span
    >
  </button>
{:else}<Popover.Root {open} onOpenChange={(next) => void opened(next)}>
    <Popover.Trigger
      bind:ref={trigger}
      class="model-trigger ui-shine ui-shine-press"
      aria-label={triggerLabel}
      disabled={!models}
    >
      {#if current}
        <span class="trigger-mark" aria-hidden="true"
          ><ProviderMark provider={current.model.provider} size={15} /></span
        >
        <span class="trigger-name">{shortName(current)}</span>
        {#if effort && levels.length > 1}<span class="trigger-level"
            ><span class="trigger-dot" aria-hidden="true">·</span>{effortName(effort)}</span
          >{/if}
      {:else}
        <span class="trigger-name empty">{m.work_model_none()}</span>
      {/if}
    </Popover.Trigger>
    <Popover.Portal>
      <Popover.Content
        class="ui-menu model-menu ui-shine-sheet"
        data-menu-material={material}
        style={`--sheet: ${sheetHeight}px`}
        side="top"
        align="end"
        customAnchor={trigger?.closest<HTMLElement>(".surface") ?? undefined}
        sideOffset={10}
        collisionPadding={12}
        aria-label={m.work_model_label()}
        onOpenAutoFocus={(event) => {
          event.preventDefault();
          const at = document.activeElement;
          if (at && main?.closest(".model-menu")?.contains(at)) return;
          (list ?? main)?.focus({ preventScroll: true });
        }}
        onCloseAutoFocus={returned}
        onEscapeKeydown={escape}
        onkeydown={keys}
      >
        <ModelRail
          {seats}
          current={shown}
          {hits}
          panel="{uid}-pane"
          onpick={(view) => void pick(view)}
        />
        <div
          class="main"
          id="{uid}-pane"
          role="tabpanel"
          tabindex="-1"
          bind:this={main}
          aria-label={searching ? m.model_search() : title}
        >
          <div class="head" data-search={searchOpen ? "" : undefined}>
            <h2 class="title">{title}</h2>
            <div class="tools">
              <button
                type="button"
                class="tool fade"
                aria-label={m.model_favourites()}
                title={m.model_favourites()}
                aria-pressed={shown === "favourites"}
                onpointerdown={(event) => event.preventDefault()}
                onclick={() => void toggleFavourites()}
                ><Icon icon={StarIcon} size={15} strokeWidth={1.8} /></button
              >
              <button
                type="button"
                class="tool fade"
                aria-label={m.model_search()}
                aria-expanded={searchOpen}
                aria-controls="{uid}-find"
                onpointerdown={(event) => event.preventDefault()}
                onclick={() => (searchOpen ? closeSearch() : void openSearch())}
                ><Icon icon={Search01Icon} size={15} strokeWidth={1.9} /></button
              >
              <button
                type="button"
                class="tool"
                aria-label={m.model_settings()}
                title={m.model_settings()}
                onpointerdown={(event) => event.preventDefault()}
                onclick={settings}
                ><Icon icon={Settings02Icon} size={15} strokeWidth={1.8} /></button
              >
            </div>
            <div class="find" id="{uid}-find" inert={!searchOpen}>
              <span class="find-icon" aria-hidden="true"
                ><Icon icon={Search01Icon} size={14} strokeWidth={2} /></span
              >
              <input
                bind:this={field}
                bind:value={query}
                type="text"
                role="combobox"
                spellcheck="false"
                autocomplete="off"
                placeholder={m.model_search()}
                aria-label={m.model_search()}
                aria-expanded="true"
                aria-controls="{uid}-list"
                aria-autocomplete="list"
                aria-activedescendant={active && items.some((item) => item.id === active)
                  ? slot(active)
                  : undefined}
                oninput={retyped}
              />
              {#if searching}<button
                  type="button"
                  class="find-clear"
                  aria-label={m.model_search_clear()}
                  onpointerdown={(event) => event.preventDefault()}
                  onclick={() => {
                    query = "";
                    active = home;
                    field?.focus();
                  }}><Icon icon={Cancel01Icon} size={10} strokeWidth={2.6} /></button
                >{/if}
            </div>
          </div>
          {#if card}
            <ModelCard
              subject={card}
              expanded={effortOpen}
              region="{uid}-effort"
              bind:pill
              ontoggle={toggleEffort}
              onEffort={chose}
              onstir={stirred}
            />
          {/if}
          {#if seat && seat.standing !== "ready" && !searching}
            <ModelAsk
              provider={seat.provider}
              name={nameOf(seat.provider)}
              standing={seat.standing}
              onact={() => step(seat)}
            />
          {:else}
            <!-- svelte-ignore a11y_click_events_have_key_events -->
            <div
              class="list"
              id="{uid}-list"
              role="listbox"
              aria-label={searching ? m.model_search() : title}
              aria-activedescendant={!searchOpen &&
              active &&
              items.some((item) => item.id === active)
                ? slot(active)
                : undefined}
              tabindex="-1"
              bind:this={list}
              onpointermove={pointed}
              onpointerleave={() => (active = home)}
              onpointerdown={(event) => event.preventDefault()}
              onclick={clicked}
            >
              {#if plate}<span
                  class="plate"
                  data-glide={plate.glide ? "" : undefined}
                  aria-hidden="true"
                  style:transform="translateY({plate.y}px)"
                  style:block-size="{plate.h}px"
                ></span>{/if}
              {#each sections as section (section.key)}
                <div
                  class="section"
                  role={spanning ? "group" : undefined}
                  aria-labelledby={spanning ? `${uid}-${section.key}` : undefined}
                >
                  {#if spanning}<div class="label" id="{uid}-{section.key}">
                      <span class="label-mark" aria-hidden="true"
                        ><ProviderMark provider={section.provider} size={12} /></span
                      ><span>{section.label}</span>
                    </div>{/if}
                  {#each section.items as item (item.id)}
                    {@const closed = locked(models, item.entry.id)}
                    {@const needed = closed ? requiredPlan(models, item.entry.id) : null}
                    {@const starry = starred.has(item.id)}
                    <div
                      class="option"
                      id={slot(item.id)}
                      role="option"
                      data-id={item.id}
                      data-active={item.id === active ? "" : undefined}
                      data-walk={item.id === active && walked ? "" : undefined}
                      aria-selected={item.id === chosen}
                      aria-label={closed
                        ? needed
                          ? m.work_model_upgrade_plan_label({
                              model: item.entry.display_name,
                              plan: planName(needed) ?? needed,
                            })
                          : m.work_model_upgrade_label({ model: item.entry.display_name })
                        : undefined}
                      aria-disabled={(!closed && !!session?.busy && session.busy !== "effort") ||
                        undefined}
                    >
                      <span class="option-name">{item.entry.display_name}</span>
                      {#if closed}<span class="upgrade" data-plan={needed ?? undefined}
                          >{needed ? (planName(needed) ?? needed) : m.work_model_upgrade()}</span
                        >{/if}
                      <span class="slot">
                        <span class="check" aria-hidden="true"
                          >{#if item.id === chosen}<Icon
                              icon={Tick02Icon}
                              size={14}
                              strokeWidth={2.2}
                            />{/if}</span
                        >
                        <button
                          type="button"
                          class="star"
                          data-on={starry ? "" : undefined}
                          tabindex="-1"
                          aria-pressed={starry}
                          aria-label={starry
                            ? m.model_unfavourite({ model: item.entry.display_name })
                            : m.model_favourite({ model: item.entry.display_name })}
                          ><Icon icon={StarIcon} size={14} strokeWidth={1.8} /></button
                        >
                      </span>
                    </div>
                  {/each}
                </div>
              {/each}
              {#if searching && !items.length}<p class="empty">
                  {m.model_no_match({ query: query.trim() })}
                </p>{:else if !searching && !items.length}<p class="empty">
                  {shown === "favourites" ? m.model_favourites_empty() : m.model_empty()}
                </p>{/if}
            </div>
          {/if}
          {#if searchOff}<p class="note">{m.work_model_no_search()}</p>{/if}
          {#if session?.fault?.action === `choose:${role}`}<p class="note failure" role="alert">
              {m.work_model_failed()}
            </p>{/if}
          {#if session?.fault?.action === "effort"}<p class="note failure" role="alert">
              {m.model_effort_failed()}
            </p>{/if}
        </div>
      </Popover.Content>
    </Popover.Portal>
  </Popover.Root>{/if}

<svelte:window
  onpointerup={() => {
    if (!held) return;
    held = false;
    rest();
  }}
  onpointerdowncapture={(event) => {
    const target = event.target instanceof Element ? event.target : null;
    if (!target) return;
    if (effortOpen) {
      if (target.closest("[role=slider]")) {
        held = true;
        clearTimeout(fold);
      } else if (!target.closest("section[data-face]")) putAway();
    }
    // An empty search that is clicked away from puts itself away.
    if (searchOpen && !query.trim() && target.closest(".model-menu") && !target.closest(".head"))
      closeSearch();
  }}
/>

<style>
  /* The chip: the shine plate, round like the + and the send button beside it. The model's mark, its name and its effort. */
  :global(.model-trigger) {
    --shine-radius: var(--radius-capsule);

    display: inline-flex;
    align-items: center;
    gap: 7px;
    max-inline-size: 300px;
    block-size: 32px;
    margin-block: 1px;
    padding-inline: 12px 14px;
    border: 0;
    color: var(--color-text);
    font: inherit;
    font-size: var(--text-label);
    line-height: 32px;
  }

  :global(.model-trigger:disabled) {
    opacity: 0;
  }

  .add-key {
    align-items: center;
    gap: 7px;
    padding-inline: 10px 12px;
    color: var(--color-label-secondary);
  }

  .trigger-mark {
    display: grid;
    flex: none;
    place-items: center;
  }

  .trigger-name {
    min-inline-size: 0;
    overflow: hidden;
    font-weight: 550;
    letter-spacing: -0.003em;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .trigger-name.empty {
    font-weight: 450;
  }

  .trigger-level {
    flex: none;
    color: var(--color-muted);
    font-weight: 450;
    white-space: nowrap;
    transition: color var(--motion-fast) var(--ease-out);
  }

  .trigger-dot {
    margin-inline-end: 5px;
    color: var(--color-faint);
  }

  :global(.model-trigger:hover) .trigger-level,
  :global(.model-trigger[data-state="open"]) .trigger-level {
    color: var(--color-label-secondary, var(--color-text));
  }

  :global(.ui-menu.model-menu) {
    display: flex;
    flex-direction: row;
    inline-size: 380px;
    block-size: min(var(--sheet, 400px), var(--bits-floating-available-height, 520px));
    max-block-size: none;
    padding: 0;
    transform-origin: var(--bits-floating-transform-origin, bottom right);
    animation:
      ui-menu-fade var(--motion-fast) var(--ease-out),
      model-menu-in var(--motion-page) var(--ease-spring);
  }

  :global(.ui-menu.model-menu[data-state="closed"]) {
    animation: ui-menu-out var(--motion-instant) var(--ease-exit) forwards;
  }

  .main {
    display: flex;
    flex: 1;
    flex-direction: column;
    min-inline-size: 0;
    min-block-size: 0;
    outline: none;
  }

  /* One row, 28px: the place's name and two small tools. The search field slides over the name. */
  .head {
    position: relative;
    display: flex;
    flex: none;
    align-items: center;
    justify-content: space-between;
    gap: 6px;
    block-size: 28px;
    margin: 10px 8px 0 14px;
  }

  .title {
    min-inline-size: 0;
    margin: 0;
    overflow: hidden;
    color: var(--color-text);
    font-size: var(--text-body);
    font-weight: 600;
    letter-spacing: -0.01em;
    text-overflow: ellipsis;
    white-space: nowrap;
    transition:
      opacity var(--motion-fast) var(--ease-out),
      translate var(--motion-base) var(--ease-out);
  }

  .head[data-search] .title {
    opacity: 0;
    translate: -6px 0;
  }

  .tools {
    display: flex;
    flex: none;
    gap: 2px;
  }

  .tool {
    display: grid;
    inline-size: 28px;
    block-size: 28px;
    padding: 0;
    border: 0;
    border-radius: 50%;
    background: transparent;
    color: var(--color-muted);
    cursor: default;
    place-items: center;
    transition:
      background-color var(--motion-fast) var(--ease-out),
      color var(--motion-fast) var(--ease-out),
      opacity var(--motion-fast) var(--ease-out),
      scale var(--motion-base) var(--ease-spring);
  }

  .tool:hover {
    background: var(--color-fill-hover);
    color: var(--color-text);
  }

  .tool:active {
    scale: 0.92;
  }

  .tool[aria-pressed="true"] {
    background: var(--row-active);
    color: var(--color-text);
  }

  .tool[aria-pressed="true"] :global(path) {
    fill: currentcolor;
  }

  .tool:focus-visible {
    outline: 2px solid var(--color-ring);
    outline-offset: -2px;
  }

  .head[data-search] .tool.fade {
    opacity: 0;
    pointer-events: none;
  }

  /* Thin, and not lit: it unrolls from the icon it replaces. */
  .find {
    position: absolute;
    inset-block: 0;
    inset-inline: 0 30px;
    display: flex;
    align-items: center;
    gap: 7px;
    padding-inline: 10px 5px;
    border-radius: var(--radius-capsule);
    background: var(--color-fill);
    box-shadow: inset 0 0 0 1px transparent;
    color: var(--color-faint);
    clip-path: inset(0 0 0 calc(100% - 28px) round 14px);
    opacity: 0;
    transition:
      clip-path var(--motion-slow) var(--ease-emphasized),
      opacity var(--motion-fast) var(--ease-exit),
      box-shadow var(--motion-fast) var(--ease-out);
  }

  .find:focus-within {
    box-shadow: var(--shadow-field-focus);
    color: var(--color-muted);
  }

  .head[data-search] .find {
    clip-path: inset(0 0 0 0 round 14px);
    opacity: 1;
    transition:
      clip-path var(--motion-slow) var(--ease-emphasized),
      opacity var(--motion-fast) var(--ease-out),
      box-shadow var(--motion-fast) var(--ease-out);
  }

  .find-icon {
    display: grid;
    flex: none;
    place-items: center;
  }

  .find input {
    flex: 1;
    min-inline-size: 0;
    padding: 0;
    border: 0;
    background: transparent;
    color: var(--color-text);
    font: inherit;
    font-size: var(--text-label);
    caret-color: var(--color-text);
    outline: none;
  }

  .find input::placeholder {
    color: var(--color-faint);
    opacity: 1;
  }

  .find-clear {
    display: grid;
    flex: none;
    inline-size: 18px;
    block-size: 18px;
    padding: 0;
    border: 0;
    border-radius: 50%;
    background: var(--row-active);
    color: var(--color-muted);
    cursor: default;
    place-items: center;
    animation: find-clear-in var(--motion-fast) var(--ease-out);
  }

  .find-clear:hover {
    background: var(--row-pressed);
    color: var(--color-text);
  }

  .find-clear:focus-visible {
    outline: 2px solid var(--color-ring);
  }

  .list {
    position: relative;
    flex: 1;
    min-block-size: 0;
    margin-block-start: 6px;
    padding: 2px 8px 12px;
    overflow-y: auto;
    overscroll-behavior: contain;
    outline: none;
    scrollbar-width: thin;
    mask-image: linear-gradient(to bottom, black calc(100% - 14px), transparent);
  }

  .plate {
    position: absolute;
    inset-block-start: 0;
    inset-inline: 8px;
    border-radius: var(--radius-row);
    background: var(--row-active);
    pointer-events: none;
  }

  .plate[data-glide] {
    transition:
      transform var(--motion-page) var(--ease-spring),
      block-size var(--motion-base) var(--ease-out);
  }

  .section + .section {
    margin-block-start: 4px;
  }

  .label {
    display: flex;
    align-items: center;
    gap: 7px;
    padding: 8px 10px 3px;
    color: var(--color-faint);
    font-size: var(--text-caption);
    font-weight: 550;
    line-height: 14px;
    letter-spacing: 0.01em;
  }

  .label-mark {
    display: grid;
    inline-size: 14px;
    place-items: center;
    opacity: 0.9;
  }

  /* One line down every row: the name, what limits it, and one place at the end for the choice's
     check, which the favourite star takes while the pointer is on the row. */
  .option {
    position: relative;
    display: flex;
    align-items: center;
    gap: 8px;
    min-block-size: 34px;
    padding-inline: 12px 6px;
    cursor: default;
  }

  .option[aria-disabled="true"] {
    opacity: 0.45;
  }

  .option-name {
    flex: 1;
    min-inline-size: 0;
    overflow: hidden;
    color: var(--color-label-secondary, var(--color-text));
    font-size: var(--text-body);
    font-weight: 450;
    line-height: 18px;
    text-overflow: ellipsis;
    white-space: nowrap;
    transition: color var(--motion-instant) var(--ease-smooth);
  }

  .option[aria-selected="true"] .option-name,
  .option[data-active] .option-name {
    color: var(--color-text);
  }

  .option[aria-selected="true"] .option-name {
    font-weight: 550;
  }

  /* A locked model names the plan that opens it; the row is the way there. */
  .upgrade {
    flex: none;
    padding: 1px 7px;
    border-radius: var(--radius-capsule);
    background: var(--color-fill);
    color: var(--color-muted);
    font-size: var(--text-caption);
    font-weight: 500;
    line-height: 14px;
  }

  .slot {
    display: grid;
    flex: none;
    inline-size: 24px;
    block-size: 24px;
    place-items: center;
  }

  .check,
  .star {
    grid-area: 1 / 1;
    transition:
      opacity var(--motion-fast) var(--ease-out),
      scale var(--motion-base) var(--ease-spring),
      color var(--motion-fast) var(--ease-out);
  }

  .check {
    display: grid;
    color: var(--color-text);
    place-items: center;
  }

  .star {
    display: grid;
    inline-size: 24px;
    block-size: 24px;
    padding: 0;
    border: 0;
    border-radius: 50%;
    background: transparent;
    color: var(--color-faint);
    cursor: default;
    opacity: 0;
    place-items: center;
    scale: 0.85;
  }

  .star:hover {
    color: var(--color-text);
  }

  /* A starred model keeps its star; the row the pointer or the arrows are on shows it too. */
  .option:hover .star,
  .option[data-walk] .star,
  .star[data-on] {
    opacity: 1;
    scale: 1;
  }

  .star[data-on] {
    color: var(--color-label-secondary);
  }

  .star[data-on] :global(path) {
    fill: currentcolor;
  }

  /* On the chosen row the star takes the check's place, and the check comes back with the pointer gone. */
  .option[aria-selected="true"] .star {
    opacity: 0;
  }

  .option[aria-selected="true"]:hover .star,
  .option[aria-selected="true"][data-walk] .star {
    opacity: 1;
  }

  .option[aria-selected="true"]:hover .check,
  .option[aria-selected="true"][data-walk] .check {
    opacity: 0;
    scale: 0.7;
  }

  .empty,
  .note {
    margin: 0;
    padding: 10px 20px;
    color: var(--color-faint);
    font-size: var(--text-label);
    line-height: 16px;
  }

  .note {
    flex: none;
  }

  .failure {
    color: var(--color-danger);
  }

  /* stylelint-disable-next-line keyframes-name-pattern -- Svelte's global prefix. */
  @keyframes -global-model-menu-in {
    from {
      transform: scale(0.94) translateY(6px);
    }
  }

  @keyframes find-clear-in {
    from {
      opacity: 0;
      scale: 0.7;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    :global(.ui-menu.model-menu),
    :global(.ui-menu.model-menu[data-state="closed"]) {
      animation: ui-menu-fade var(--motion-instant) linear;
    }

    .plate[data-glide],
    .find,
    .title,
    .star {
      transition: none;
    }

    .find-clear {
      animation: none;
    }
  }

  @media (forced-colors: active) {
    .plate {
      outline: 1px solid Highlight;
    }
  }
</style>
