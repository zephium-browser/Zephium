<script lang="ts">
  import * as m from "$shared/i18n/messages";
  import {
    Alert02Icon,
    ArrowDown01Icon,
    ArrowUp01Icon,
    Cancel01Icon,
    Search01Icon,
  } from "@hugeicons/core-free-icons";
  import { settle } from "$domain/operations";
  import { extensions } from "$domain/extensions";
  import { webext } from "$domain/webext";
  import { tabs } from "$domain/tabs";
  import { surface } from "$domain/surface";
  import { uiCommands as ui } from "$domain/ui-commands";
  import { isHidden, isPeeking, peek } from "$session/sidebar-mode.svelte";
  import { commands } from "$shared/ipc/bindings";
  import Icon from "$shared/ui/Icon";
  import Button from "$shared/ui/Button";
  import IconButton from "$shared/ui/IconButton";
  import CaptureControl from "$shared/ui/CaptureControl";
  import { preferences } from "$domain/preferences";
  import * as find from "../lib/find.svelte";
  import { flushSync, tick, untrack, type Snippet } from "svelte";
  import { duration, easing, reducedMotion } from "$shared/lib/motion";
  import {
    addressSecurity,
    editingAddress,
    restingAddress,
    type SitePage,
  } from "../lib/address-model";

  let {
    compact = false,
    trailing,
  }: {
    compact?: boolean;
    /** Controls that act on the page, at the far end of the field. Told which
     *  page it is, or null where there is no site to describe. */
    trailing?: Snippet<[SitePage | null]>;
  } = $props();

  let input: HTMLInputElement;
  let findInput: HTMLInputElement | undefined = $state();
  let finding = $derived(find.isOpen());
  let findCount = $derived.by(() => {
    const matches = find.count();
    if (matches === null) return "";
    if (matches === 0) return m.find_none();
    const at = find.position();
    return at === null ? String(matches) : m.find_position({ at, of: matches });
  });

  // Whichever field is mounted takes focus when find asks for it, including
  // the one that appears when a compact sidebar opens for the search.
  $effect(() => {
    if (find.focusRequested() === 0 || !finding || !findInput) return;
    untrack(() => {
      findInput?.focus();
      findInput?.select();
    });
  });
  let form: HTMLFormElement;
  let editing = $state(false);
  let pending = $state(false);
  let failed = $state(false);
  let composing = false;
  let draft = $state("");
  let activeUrl = $derived(tabs.activeTab()?.url ?? "");
  let activeContent = $derived(tabs.activeTab()?.content ?? "web");
  // The page in front using the camera or microphone says so where its
  // address is, as well as on its tab.
  let activeCapture = $derived(tabs.activeTab()?.capture ?? null);
  // Only a store page actually in front, never one behind Settings or another
  // browser page, offers to install.
  let listedId = $derived(
    webext.isAvailable() && surface.currentPage() === null
      ? extensions.chromeStoreListingId(activeUrl)
      : null,
  );
  let listedInstalled = $derived(listedId === null ? null : webext.named(listedId));
  let removing = $state(false);
  $effect(() => {
    void listedId;
    removing = false;
  });
  let placeholder = $derived(
    activeContent === "extension_owned"
      ? "Extension page — enter an address to open a new tab"
      : m.ui_enter_an_address(),
  );
  let zoom = $derived(tabs.activeTab()?.zoom ?? null);
  let standardZoom = $derived(Number(preferences.value("zoom.default")));
  let authoritativeValue = $derived(restingAddress(activeUrl));
  let value = $derived(editing ? draft : authoritativeValue);
  let security = $derived(
    addressSecurity(activeUrl, tabs.activeTab()?.certificate_exception === true),
  );
  // A web page in front has a site to describe. A new tab, an internal page
  // and a page behind Settings have none.
  let sitePage = $derived<SitePage | null>(
    activeContent === "web" && security !== "none" && surface.currentPage() === null
      ? {
          url: activeUrl,
          security,
          warning:
            security === "untrusted"
              ? m.address_certificate_exception()
              : security === "insecure"
                ? m.address_insecure()
                : null,
          capture: activeCapture,
          privateWindow: tabs.profile()?.kind === "incognito",
          zoom,
          standardZoom,
        }
      : null,
  );

  $effect(() => {
    const command = ui.uiCommand();
    if (command.seq === 0 || command.id !== "url.focus" || input === undefined) return;
    // At rail width the field is not drawn; the launcher stands in for it.
    const tab = untrack(() => tabs.activeTab());
    // A hidden column is not on screen either, unless it has been called up.
    const offscreen = untrack(() => isHidden() && !isPeeking());
    if ((compact || offscreen) && tab?.url && (tab.content ?? "web") === "web") {
      // Work has no page in front, so there the launcher opens empty.
      if (untrack(() => surface.currentPage()) === "work")
        void commands.runCommand("launcher.toggle");
      else void commands.launcherEdit(tab.id, tab.url);
      return;
    }
    // Without a page to edit, the field itself is wanted: the hidden column
    // floats up first, since its fields take no focus while it is away. The
    // focused field then holds it up until the person moves on. A rail has
    // no field to show, so there the launcher stands in, as above.
    if (offscreen && compact) {
      void commands.runCommand("launcher.toggle");
      return;
    }
    if (offscreen) {
      const field = input;
      void peek().then(async (onSide) => {
        if (onSide === null) return;
        await tick();
        field.focus();
        field.select();
      });
      return;
    }
    input.focus();
    input.select();
  });

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    if (composing || pending || !value.trim()) return;
    const id = tabs.activeId();
    const profile = tabs.profile()?.id;
    const opensNewTab = activeContent !== "web";
    const submitted = value;
    pending = true;
    failed = false;
    try {
      const result = await settle(
        id === null
          ? commands.browserOpenUrl(submitted, false)
          : commands.tabsNavigate(id, submitted),
      );
      if (tabs.profile()?.id !== profile) return;
      if (id !== null && tabs.activeId() !== id) {
        if (opensNewTab && result.outcome !== "failed" && result.outcome !== "rejected")
          input.blur();
        return;
      }
      if (result.outcome === "failed" || result.outcome === "rejected") failed = true;
      else if (value === submitted) input.blur();
    } catch {
      if (tabs.activeId() === id) failed = true;
    } finally {
      pending = false;
    }
  }

  function handleInput(event: Event) {
    const target = event.currentTarget;
    if (!(target instanceof HTMLInputElement)) return;
    failed = false;
    draft = target.value;
  }

  function beginEditing() {
    // Keep the resting chrome quiet while preserving the complete location
    // when the user explicitly enters edit mode.
    const from = textStart();
    draft = editingAddress(activeUrl) || value;
    editing = true;
    flushSync();
    input.select();
    slide(from);
  }

  function endEditing() {
    const from = textStart();
    editing = false;
    flushSync();
    slide(from);
  }

  let ruler: CanvasRenderingContext2D | null = null;

  /** Where the text visibly begins inside the input, in its current shape. */
  function textStart(): number {
    const style = getComputedStyle(input);
    ruler ??= document.createElement("canvas").getContext("2d");
    if (!ruler) return 0;
    ruler.font = style.font;
    const text = input.value || input.placeholder;
    const width = ruler.measureText(text).width;
    const start = Number.parseFloat(style.paddingInlineStart) || 0;
    const end = Number.parseFloat(style.paddingInlineEnd) || 0;
    const room = input.clientWidth - start - end;
    return style.textAlign === "center" ? start + Math.max(0, (room - width) / 2) : start;
  }

  // Entering and leaving the field is one line of text moving between its
  // resting place, centred, and its working place at the start. The text is
  // swapped at the same moment — the host for the whole address — so it
  // travels half-lit and brightens as it lands. Transform only; the clip
  // around the input keeps it inside the field while it moves.
  function slide(from: number) {
    const offset = from - textStart();
    if (Math.abs(offset) < 1 || reducedMotion()) return;
    input.animate(
      [
        { transform: `translateX(${offset}px)`, opacity: 0.5 },
        { transform: "none", opacity: 1 },
      ],
      { duration: duration("slow"), easing: easing("emphasized") },
    );
  }

  // Editing ends wherever attention goes next: a press anywhere else in the
  // chrome, or the page itself taking focus away from the chrome entirely.
  $effect(() => {
    if (!editing) return;
    const press = (event: PointerEvent) => {
      if (!(event.target instanceof Node) || !form.contains(event.target)) input.blur();
    };
    const away = () => input.blur();
    window.addEventListener("pointerdown", press, true);
    window.addEventListener("blur", away);
    return () => {
      window.removeEventListener("pointerdown", press, true);
      window.removeEventListener("blur", away);
    };
  });

  let pageRequest = $derived(tabs.activeTab()?.page_request ?? null);

  // What appears only on demand is fetched the first time it is wanted, not
  // at startup; a failed fetch is tried again at the next request.
  let RequestCard = $state.raw<typeof import("./PageRequestCard.svelte").default | null>(null);
  $effect(() => {
    if (pageRequest && RequestCard === null)
      void import("./PageRequestCard.svelte").then(
        (module) => (RequestCard = module.default),
        () => {},
      );
  });
</script>

<!--
  The input stays mounted in both shapes. Rust's synchronous presentation
  barrier commits and verifies the authoritative host through this exact
  element before revealing page content, so unmounting it at rail width would
  reject every presentation and leave page content permanently concealed. In
  compact mode it is present and correct but unpainted, and the launcher is the
  visible way to reach it.
-->
<form bind:this={form} class="shrink-0" class:pb-2={!compact} onsubmit={submit} role="search">
  <!--
    A refused navigation rings the field, following the shared Field
    convention. The message is announced rather than drawn: a block of text
    appearing under the address bar would push the whole chrome down.
  -->
  <div
    class:sr-only={compact}
    class:flex={!compact}
    class:shadow-[inset_0_0_0_1px_var(--color-danger)]={failed}
    class="address-field h-[34px] items-center gap-2 rounded-row bg-address ps-2.5 pe-1 shadow-[var(--shadow-tile)] transition-[background-color,box-shadow] duration-[var(--motion-fast)] ease-[var(--ease-out)] [--address-centering:20px] focus-within:bg-address-hover focus-within:shadow-[var(--shadow-field-focus)] hover:bg-address-hover"
  >
    {#if !compact && activeCapture && !editing}
      <CaptureControl
        site={activeUrl}
        capture={activeCapture}
        onOpen={(x, y) => {
          const id = tabs.activeId();
          if (id !== null) tabs.openMediaMenu(id, x, y);
        }}
      />
    {/if}

    <!--
      Resting, the text centres on the field rather than on the input. The
      trailing tray takes 26px and the field's own gutters differ by 6px, so
      the input's centre sits 10px left of the field's; padding twice that on
      the leading edge puts it back. Editing drops the padding and returns to
      the start, where a long URL has to begin.
    -->
    <span class="address-clip">
      <!-- Stays mounted and correct while find borrows the field: native
           verifies the page's address through it before showing the page.
           At rest a host too long to fit loses its start, not its end, so
           the site that owns it stays visible: right-to-left only clips. -->
      <input
        bind:this={input}
        class:sr-only={finding}
        tabindex={finding ? -1 : undefined}
        aria-hidden={finding || undefined}
        data-zephium-address
        type="text"
        aria-label={m.ui_address_and_search()}
        autocomplete="off"
        autocapitalize="off"
        enterkeyhint="go"
        {placeholder}
        spellcheck="false"
        {value}
        oninput={handleInput}
        oncompositionstart={() => (composing = true)}
        oncompositionend={() => (composing = false)}
        onkeydown={(event) => {
          if (event.key === "Escape" && !event.isComposing) {
            event.preventDefault();
            failed = false;
            input.blur();
          }
        }}
        aria-invalid={failed || undefined}
        aria-describedby={failed ? "address-error" : undefined}
        onfocus={beginEditing}
        onblur={endEditing}
        style:text-align={editing ? "start" : "center"}
        style:direction={editing ? null : "rtl"}
        style:padding-left={editing ? "0" : "var(--address-centering)"}
        class="min-w-0 flex-1 bg-transparent text-[13.5px] text-ellipsis text-label-secondary outline-none placeholder:text-faint focus:text-text"
      />
      {#if finding}
        <span class="find-glyph" aria-hidden="true"><Icon icon={Search01Icon} size={14} /></span>
        <input
          bind:this={findInput}
          type="search"
          class="find-input"
          aria-label={m.find_placeholder()}
          placeholder={m.find_placeholder()}
          autocomplete="off"
          spellcheck="false"
          enterkeyhint="search"
          value={find.text()}
          oninput={(event) => find.setText(event.currentTarget.value)}
          onkeydown={(event) => {
            if (event.isComposing) return;
            if (event.key === "Enter") {
              event.preventDefault();
              find.step(!event.shiftKey, tabs.activeId());
            } else if (event.key === "Escape") {
              event.preventDefault();
              find.hide();
            }
          }}
        />
      {/if}
    </span>

    {#if finding}
      <span class="find-count" role="status" aria-live="polite">{findCount}</span>
      <IconButton
        icon={ArrowUp01Icon}
        label={m.find_previous()}
        size={14}
        buttonSize={24}
        disabled={!find.count()}
        onclick={() => find.step(false, tabs.activeId())}
      />
      <IconButton
        icon={ArrowDown01Icon}
        label={m.find_next()}
        size={14}
        buttonSize={24}
        disabled={!find.count()}
        onclick={() => find.step(true, tabs.activeId())}
      />
      <IconButton
        icon={Cancel01Icon}
        label={m.find_done()}
        size={14}
        buttonSize={24}
        onclick={() => find.hide()}
      />
    {:else if !compact}
      {#if trailing}{@render trailing(sitePage)}{/if}
    {/if}
  </div>
  {#if !compact && listedInstalled}
    {#if removing}
      <div
        class="store-remove"
        role="group"
        aria-label={m.webext_remove_confirm({ name: listedInstalled.name })}
      >
        <span>{m.webext_remove_confirm({ name: listedInstalled.name })}</span>
        <Button size="compact" variant="secondary" onclick={() => (removing = false)}
          >{m.webext_keep()}</Button
        >
        <Button
          size="compact"
          variant="danger"
          onclick={() => {
            removing = false;
            if (listedInstalled) void webext.uninstall(listedInstalled.id);
          }}>{m.webext_remove()}</Button
        >
      </div>
    {:else}
      <Button
        variant="secondary"
        size="compact"
        class="mt-2 w-full"
        onclick={() => (removing = true)}>{m.webext_store_remove()}</Button
      >
    {/if}
  {:else if !compact && listedId !== null}
    <Button
      variant="primary"
      size="compact"
      class="mt-2 w-full"
      pending={webext.isInstalling()}
      data-extension-store-install
      title="Install this extension in Zephium"
      onclick={() => {
        const id = tabs.activeId();
        if (id !== null) void webext.prepare(id);
      }}
    >
      {webext.isPreparing()
        ? "Preparing…"
        : webext.isInstalling()
          ? m.webext_store_installing()
          : m.webext_store_install()}
    </Button>
    {#if webext.error() !== null && webext.review() === null}
      <p class="mt-1 text-[11px] leading-4 text-danger" role="alert">{webext.error()}</p>
    {/if}
  {/if}
  {#if failed}<p id="address-error" role="alert" class="sr-only">{m.browser_nav_failed()}</p>{/if}
  {#if pageRequest && compact}
    <p class="popup-notice" role="status" title={m.page_request_waiting()}>
      <Icon icon={Alert02Icon} size={14} />
    </p>
  {:else if pageRequest && RequestCard}
    {#key `${tabs.activeId()}:${JSON.stringify(pageRequest)}`}
      <RequestCard tab={tabs.activeId()!} request={pageRequest} />
    {/key}
  {/if}
</form>

<style>
  .store-remove {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-block-start: 8px;
    color: var(--color-label-secondary);
    font-size: 11px;
    line-height: 14px;
  }

  .store-remove > span {
    flex: 1;
    min-inline-size: 0;
  }

  .popup-notice {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    margin: 8px 0 0;
    color: var(--color-label-secondary);
    font-size: 11px;
  }

  .find-glyph {
    display: grid;
    place-items: center;
    flex: none;
    margin-inline-end: 6px;
    color: var(--color-faint);
  }

  .find-input {
    flex: 1;
    min-width: 0;
    border: 0;
    background: transparent;
    color: var(--color-text);
    font: inherit;
    font-size: 13.5px;
    outline: none;
    user-select: text;
  }

  .find-input::placeholder {
    color: var(--color-faint);
  }

  .find-input::-webkit-search-cancel-button {
    display: none;
  }

  .find-count {
    flex: none;
    font-size: 11.5px;
    font-variant-numeric: tabular-nums;
    color: var(--color-faint);
    white-space: nowrap;
  }

  /* The input's travel is clipped here rather than by the field, whose tray
     opens a panel below it that must not be cut off. */
  .address-clip {
    display: flex;
    flex: 1;
    min-width: 0;
    align-self: stretch;
    overflow: clip;
  }
</style>
