<script lang="ts">
  import Button from "$shared/ui/Button";
  import Icon from "$shared/ui/Icon";
  import * as m from "$shared/i18n/messages";
  import AskReceipt from "./AskReceipt.svelte";
  import AskShell from "./AskShell.svelte";
  import type { AddressAsk } from "./asks";
  import { BrowserIcon } from "./icons";

  /**
   * "Open this address?": one the agent wrote itself after reading the
   * person's own information, where a page could have told it to carry that
   * information out. The whole address is shown, its host first.
   */
  let {
    ask,
    placement = "canvas",
    busy = false,
    onanswer,
  }: {
    ask: AddressAsk;
    placement?: "canvas" | "island";
    busy?: boolean;
    /** Rust's own option words go back as they came. */
    onanswer: (answer: string) => void;
  } = $props();

  const title = m.work_ask_address_title();
  /** The address split around its host, so the host reads first. */
  const parts = $derived.by(() => {
    const at = ask.url.indexOf(ask.host);
    return at < 0
      ? { before: "", host: "", after: ask.url }
      : {
          before: ask.url.slice(0, at),
          host: ask.host,
          after: ask.url.slice(at + ask.host.length),
        };
  });
  const opened = $derived(ask.answer === ask.open || ask.answer === ask.allowSite);
</script>

{#if ask.state === "open"}
  <AskShell
    {placement}
    {busy}
    label={title}
    where={ask.host}
    {title}
    note={m.work_ask_address_note()}
  >
    {#snippet mark()}<Icon icon={BrowserIcon} size={16} />{/snippet}
    <p class="address">
      <span class="faint">{parts.before}</span><strong>{parts.host}</strong><span
        >{parts.after}</span
      >
    </p>
    {#snippet actions()}
      <Button variant="ghost" disabled={busy} onclick={() => onanswer(ask.decline)}
        >{m.work_ask_address_decline()}</Button
      >
      <Button variant="ghost" disabled={busy} onclick={() => onanswer(ask.allowSite)}
        >{m.work_ask_address_allow_site()}</Button
      >
      <Button variant="primary" pending={busy} onclick={() => onanswer(ask.open)}
        >{m.work_ask_address_open()}</Button
      >
    {/snippet}
  </AskShell>
{:else if ask.answer}
  <AskReceipt
    tone={opened ? "done" : "declined"}
    status={opened ? m.work_ask_address_opened() : m.work_ask_address_not_opened()}
    text={ask.host}
  />
{/if}

<style>
  .address {
    max-block-size: 96px;
    margin: 0;
    padding: 8px 10px;
    overflow-y: auto;
    border-radius: var(--radius-row);
    background: var(--color-fill);
    color: var(--color-label-secondary);
    font-family: var(--font-mono);
    font-size: var(--text-caption);
    line-height: 16px;
    overflow-wrap: anywhere;
    user-select: text;
  }

  .address strong {
    color: var(--color-text);
    font-weight: 600;
  }

  .faint {
    color: var(--color-faint);
  }
</style>
