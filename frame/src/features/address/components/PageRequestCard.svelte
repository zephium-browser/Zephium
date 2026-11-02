<script lang="ts">
  import { Alert02Icon, LinkSquare02Icon, LockIcon } from "@hugeicons/core-free-icons";
  import * as m from "$shared/i18n/messages";
  import type { PageRequestAnswer, PageRequestView } from "$shared/ipc/bindings";
  import { commands } from "$shared/ipc/bindings";
  import Icon from "$shared/ui/Icon";
  import Button from "$shared/ui/Button";
  import Field from "$shared/ui/Field";

  let { tab, request }: { tab: string; request: PageRequestView } = $props();
  let always = $state(false);
  let username = $state("");
  let password = $state("");
  let fields = $state<HTMLDivElement>();

  // Signing in: the site, or the proxy in front of it, asks for a username
  // and password. Only the host is shown; the server's own text never is.
  let auth = $derived(request.kind === "http_auth" ? request : null);
  $effect(() => {
    if (auth) fields?.querySelector("input")?.focus();
  });

  function signIn() {
    if (!auth || !username) return;
    const credentials = { username, password };
    username = "";
    password = "";
    void commands.tabsAnswerSignIn(tab, auth.request, credentials).catch(() => {});
  }

  function cancelSignIn() {
    if (!auth) return;
    password = "";
    void commands.tabsAnswerSignIn(tab, auth.request, null).catch(() => {});
  }

  // The card sits inside the address form, so Enter must not submit that.
  function signInKey(event: KeyboardEvent) {
    if (event.isComposing) return;
    if (event.key === "Enter") {
      event.preventDefault();
      event.stopPropagation();
      signIn();
    } else if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      cancelSignIn();
    }
  }

  // A link for another app, and a new tab the page could not open, wait here
  // under the field, in chrome the page cannot draw over, until answered.
  let app = $derived(request.kind === "external_app" ? request : null);
  // A hung page: the person may wait for it or reload it in a fresh view.
  let hung = $derived(request.kind === "unresponsive");
  let title = $derived(
    auth
      ? auth.proxy
        ? m.page_request_sign_in_proxy_title({ host: auth.host })
        : m.page_request_sign_in_title({ host: auth.host })
      : app
        ? app.app
          ? m.page_request_app_title({ app: app.app })
          : m.page_request_app_missing({ scheme: app.scheme })
        : hung
          ? m.page_request_unresponsive_title()
          : m.page_request_popup_title(),
  );
  let detail = $derived(
    app
      ? app.app
        ? app.site
          ? m.page_request_app_detail({ site: app.site })
          : m.page_request_app_detail_page()
        : null
      : hung
        ? m.page_request_unresponsive_detail()
        : request.kind === "popup" && request.host
          ? request.host
          : null,
  );
  let openable = $derived(
    app ? app.app !== null : request.kind === "popup" && request.host !== null,
  );
  // The site asking, when its profile can keep allowing its pop-ups.
  let popupSite = $derived(request.kind === "popup" ? request.site : null);

  function answer(value: PageRequestAnswer) {
    void commands.tabsAnswerPageRequest(tab, value).catch(() => {});
  }
</script>

<div class="page-request" role="alertdialog" aria-label={title} data-zephium-page-request>
  <div class="head">
    <Icon icon={auth ? LockIcon : app ? LinkSquare02Icon : Alert02Icon} size={14} />
    <div class="words">
      <p class="title">{title}</p>
      {#if detail}<p class="detail">{detail}</p>{/if}
    </div>
  </div>
  {#if auth}
    {#if auth.retry}
      <p class="notice" role="alert">{m.page_request_sign_in_retry()}</p>
    {/if}
    {#if auth.insecure}
      <p class="notice">{m.page_request_sign_in_insecure()}</p>
    {/if}
    <div class="fields" bind:this={fields}>
      <Field
        label={m.page_request_sign_in_username()}
        bind:value={username}
        autocomplete="off"
        autocapitalize="off"
        spellcheck={false}
        onkeydown={signInKey}
      />
      <Field
        label={m.page_request_sign_in_password()}
        type="password"
        bind:value={password}
        autocomplete="off"
        onkeydown={signInKey}
      />
    </div>
    <div class="actions">
      <Button size="compact" variant="ghost" onclick={cancelSignIn}>
        {m.page_request_cancel()}
      </Button>
      <Button size="compact" variant="primary" disabled={!username} onclick={signIn}>
        {m.page_request_sign_in_submit()}
      </Button>
    </div>
  {:else if hung}
    <div class="actions">
      <Button size="compact" variant="ghost" onclick={() => answer("dismiss")}>
        {m.page_request_wait()}
      </Button>
      <Button size="compact" variant="primary" onclick={() => answer("allow")}>
        {m.navigation_error_reload()}
      </Button>
    </div>
  {:else}
    {#if app?.app && app.site}
      <label class="always">
        <input type="checkbox" bind:checked={always} />
        {m.page_request_app_always({ site: app.site })}
      </label>
    {:else if popupSite}
      <label class="always">
        <input type="checkbox" bind:checked={always} />
        {m.page_request_popup_always({ site: popupSite })}
      </label>
    {/if}
    <div class="actions">
      <Button
        size="compact"
        variant="ghost"
        onclick={() => answer(popupSite && always && !openable ? "always_allow" : "dismiss")}
      >
        {openable ? m.page_request_cancel() : m.page_request_dismiss()}
      </Button>
      {#if openable}
        <Button
          size="compact"
          variant="primary"
          onclick={() => answer((app || popupSite) && always ? "always_allow" : "allow")}
        >
          {m.page_request_open()}
        </Button>
      {/if}
    </div>
  {/if}
</div>

<style>
  .page-request {
    display: grid;
    gap: 8px;
    margin-block-start: 8px;
    padding: 10px;
    border-radius: var(--radius-row);
    background: var(--color-fill);
    color: var(--color-text);
  }

  .head {
    display: flex;
    gap: 8px;
    align-items: flex-start;
  }

  .head > :global(svg) {
    flex: none;
    margin-block-start: 2px;
    color: var(--color-muted);
  }

  .words {
    min-inline-size: 0;
  }

  .title {
    margin: 0;
    font-size: 12px;
    font-weight: 500;
    line-height: 16px;
    overflow-wrap: anywhere;
  }

  .detail {
    margin: 0;
    color: var(--color-muted);
    font-size: 11px;
    line-height: 14px;
    overflow-wrap: anywhere;
  }

  .notice {
    margin: 0;
    color: var(--color-danger);
    font-size: 11px;
    line-height: 14px;
  }

  .fields {
    display: grid;
    gap: 6px;
  }

  .always {
    display: flex;
    gap: 6px;
    align-items: center;
    color: var(--color-muted);
    font-size: 11px;
    line-height: 14px;
  }

  .actions {
    display: flex;
    gap: 6px;
    justify-content: flex-end;
  }
</style>
