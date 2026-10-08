<script lang="ts">
  import { Alert02Icon, LinkSquare02Icon } from "@hugeicons/core-free-icons";
  import * as m from "$shared/i18n/messages";
  import type { PageRequestAnswer, PageRequestView } from "$shared/ipc/bindings";
  import { commands } from "$shared/ipc/bindings";
  import Icon from "$shared/ui/Icon";
  import Button from "$shared/ui/Button";

  let { tab, request }: { tab: string; request: PageRequestView } = $props();
  let always = $state(false);

  // A link for another app, and a new tab the page could not open, wait here
  // under the field, in chrome the page cannot draw over, until answered.
  let app = $derived(request.kind === "external_app" ? request : null);
  let title = $derived(
    app
      ? app.app
        ? m.page_request_app_title({ app: app.app })
        : m.page_request_app_missing({ scheme: app.scheme })
      : m.page_request_popup_title(),
  );
  let detail = $derived(
    app
      ? app.app
        ? app.site
          ? m.page_request_app_detail({ site: app.site })
          : m.page_request_app_detail_page()
        : null
      : request.kind === "popup" && request.host
        ? request.host
        : null,
  );
  let openable = $derived(
    app ? app.app !== null : request.kind === "popup" && request.host !== null,
  );

  function answer(value: PageRequestAnswer) {
    void commands.tabsAnswerPageRequest(tab, value).catch(() => {});
  }
</script>

<div class="page-request" role="alertdialog" aria-label={title} data-zephium-page-request>
  <div class="head">
    <Icon icon={app ? LinkSquare02Icon : Alert02Icon} size={14} />
    <div class="words">
      <p class="title">{title}</p>
      {#if detail}<p class="detail">{detail}</p>{/if}
    </div>
  </div>
  {#if app?.app && app.site}
    <label class="always">
      <input type="checkbox" bind:checked={always} />
      {m.page_request_app_always({ site: app.site })}
    </label>
  {/if}
  <div class="actions">
    <Button size="compact" variant="ghost" onclick={() => answer("dismiss")}>
      {openable ? m.page_request_cancel() : m.page_request_dismiss()}
    </Button>
    {#if openable}
      <Button
        size="compact"
        variant="primary"
        onclick={() => answer(app && always ? "always_allow" : "allow")}
      >
        {m.page_request_open()}
      </Button>
    {/if}
  </div>
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
