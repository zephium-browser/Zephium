<script lang="ts">
  import { onMount, tick } from "svelte";
  import { commands } from "$shared/ipc/bindings";
  import type { WorkLocalServerV1, WorkModelProvider } from "$shared/ipc/bindings";
  import { compatiblePresets, type ModelsSession, keyState, providerName } from "$domain/ai";
  import ProviderMark from "$shared/ui/ProviderMark";
  import Button from "$shared/ui/Button";
  import Field from "$shared/ui/Field";
  import Icon from "$shared/ui/Icon";
  import Menu from "$shared/ui/Menu";
  import MoreHorizontalIcon from "@hugeicons/core-free-icons/MoreHorizontalIcon";
  import * as m from "$shared/i18n/messages";

  let { session, provider }: { session: ModelsSession; provider: WorkModelProvider } = $props();

  const custom = $derived(provider === "compatible");
  const name = $derived(custom ? m.ai_custom_name() : providerName(provider));
  const models = $derived(session.models);
  const key = $derived(keyState(models, provider));
  const base = $derived(models?.providers.find((row) => row.provider === provider)?.base ?? null);
  const providerFault = $derived(models?.providers.find((row) => row.provider === provider)?.fault);
  const keyUrls: Partial<Record<WorkModelProvider, string>> = {
    anthropic: "https://platform.claude.com/settings/keys",
    open_ai: "https://platform.openai.com/api-keys",
    google: "https://aistudio.google.com/apikey",
    deep_seek: "https://platform.deepseek.com/api_keys",
    open_router: "https://openrouter.ai/settings/keys",
    xai: "https://console.x.ai",
    mistral: "https://console.mistral.ai/api-keys",
    groq: "https://console.groq.com/keys",
    cerebras: "https://cloud.cerebras.ai",
  };
  const keyUrl = $derived(keyUrls[provider]);
  const configured = $derived(custom ? !!base : key !== "missing");
  const description = $derived(
    {
      anthropic: m.ai_provider_anthropic,
      open_ai: m.ai_provider_open_ai,
      google: m.ai_provider_google,
      deep_seek: m.ai_provider_deep_seek,
      open_router: m.ai_provider_open_router,
      xai: m.ai_provider_xai,
      mistral: m.ai_provider_mistral,
      groq: m.ai_provider_groq,
      cerebras: m.ai_provider_cerebras,
      compatible: m.ai_provider_compatible,
      cloud: m.ai_provider_compatible,
    }[provider](),
  );

  /** Model servers already running here, offered in one click. */
  let local = $state.raw<WorkLocalServerV1[]>([]);
  onMount(() => {
    if (!custom) return;
    let live = true;
    void Promise.resolve()
      .then(() => commands.workLocalServers())
      .then((found) => {
        if (live) local = found;
      })
      .catch(() => {});
    return () => (live = false);
  });
  const ollama = $derived(!!base && /^http:\/\/(localhost|127\.0\.0\.1):11434\b/u.test(base));

  async function useLocal(server: WorkLocalServerV1) {
    message = null;
    if (!(await session.setEndpoint(server.base)))
      message = { text: explain(session.fault?.fault), tone: "error" };
  }

  let editing = $state(false);
  let secret = $state("");
  let address = $state("");
  let input = $state<HTMLElement>();
  let message = $state<{ text: string; tone: "error" | "done" } | null>(null);

  const saving = $derived(
    session.busy === `key:${provider}` || (custom && session.busy === "endpoint"),
  );
  const testing = $derived(session.busy === `test:${provider}`);

  const status = $derived.by(() => {
    if (custom) return base ?? null;
    if (key === "valid") return { text: m.ai_key_accepted(), tone: "valid" as const };
    if (key === "invalid")
      return { text: m.ai_key_invalid({ provider: name }), tone: "invalid" as const };
    if (key === "set") return { text: m.ai_key_set(), tone: "set" as const };
    return null;
  });

  function explain(fault: string | undefined) {
    switch (fault) {
      case "key_refused":
        return m.ai_key_refused({ provider: name });
      case "billing":
        return m.ai_fault_billing();
      case "rate_limited":
        return m.ai_fault_rate_limit();
      case "provider_down":
        return m.ai_fault_provider_down();
      case "request":
        return m.ai_fault_request();
      case "keychain":
        return m.ai_key_keychain();
      case "keychain_waiting":
        return m.ai_key_keychain_waiting();
      case "invalid":
        return custom ? m.ai_endpoint_invalid() : m.ai_key_refused({ provider: name });
      default:
        return m.ai_key_unreachable({ provider: name });
    }
  }

  async function edit() {
    message = null;
    secret = "";
    address = base ?? "";
    editing = true;
    await tick();
    input?.querySelector("input")?.focus();
  }

  function escape(event: KeyboardEvent) {
    if (event.key !== "Escape") return;
    event.preventDefault();
    cancel();
  }

  function cancel() {
    editing = false;
    secret = "";
    message = null;
  }

  async function save() {
    message = null;
    if (custom) {
      if (!(await session.setEndpoint(address.trim() || null))) {
        message = { text: explain(session.fault?.fault), tone: "error" };
        return;
      }
      if (secret.trim() && !(await session.setKey(provider, secret))) {
        message = { text: explain(session.fault?.fault), tone: "error" };
        return;
      }
    } else if (!(await session.setKey(provider, secret))) {
      message = { text: explain(session.fault?.fault), tone: "error" };
      return;
    }
    secret = "";
    editing = false;
  }

  async function test() {
    message = null;
    const passed = await session.testKey(provider);
    message = passed
      ? { text: m.ai_key_accepted(), tone: "done" }
      : { text: explain(session.fault?.fault), tone: "error" };
  }

  async function remove() {
    message = null;
    if (custom) {
      await session.setEndpoint(null);
      if (key !== "missing") await session.clearKey(provider);
    } else if (!(await session.clearKey(provider))) {
      message = { text: explain(session.fault?.fault), tone: "error" };
    }
  }
</script>

<div class="ai-row" data-editing={editing}>
  <div class="head">
    <span class="tile" aria-hidden="true"><ProviderMark {provider} size={20} /></span>
    <div class="copy">
      <h3>{name}</h3>
      <p>
        {#if typeof status === "string"}<span class="address">{status}</span>{:else if status}<span
            class="status"
            data-tone={status.tone}><span class="dot" aria-hidden="true"></span>{status.text}</span
          >{:else}{description}{/if}
      </p>
    </div>
    {#if !editing}
      <div class="actions">
        {#if !configured}
          <Button size="compact" onclick={() => void edit()}
            >{custom ? m.ai_endpoint_set_up() : m.ai_key_add()}</Button
          >
        {:else}
          {#if !custom}<Button
              size="compact"
              variant="ghost"
              pending={testing}
              disabled={!!session.busy}
              onclick={() => void test()}>{m.ai_key_test()}</Button
            >{/if}
          <Menu
            label={m.ai_key_options({ provider: name })}
            triggerClass="ai-row-more"
            align="end"
            entries={[
              {
                kind: "item",
                id: "replace",
                label: custom ? m.ai_endpoint_edit() : m.ai_key_replace(),
              },
              {
                kind: "item",
                id: "remove",
                label: custom ? m.ai_endpoint_remove() : m.ai_key_remove(),
                danger: true,
              },
            ]}
            onselect={(id) => (id === "replace" ? void edit() : void remove())}
          >
            {#snippet trigger()}<Icon icon={MoreHorizontalIcon} size={16} />{/snippet}
          </Menu>
        {/if}
      </div>
    {/if}
  </div>
  {#if !configured && keyUrl}<p class="message">
      <a
        href={keyUrl}
        onclick={(event) => {
          event.preventDefault();
          void commands.browserOpenUrl(keyUrl, true).catch(() => {
            message = { text: m.ai_link_failed(), tone: "error" };
          });
        }}>{m.ai_get_key({ provider: name })}</a
      >
    </p>{/if}
  {#if custom && !configured && !editing && local.length}<div class="local">
      {#each local as server (server.kind)}<Button
          size="compact"
          disabled={!!session.busy}
          onclick={() => void useLocal(server)}
          >{server.kind === "ollama" ? m.ai_local_use_ollama() : m.ai_local_use_lm_studio()}</Button
        >{/each}
    </div>{/if}
  {#if custom && ollama && !editing}<p class="message">{m.ai_local_ollama_context()}</p>{/if}
  {#if providerFault && !message}<p class="message" role="status">{explain(providerFault)}</p>{/if}
  {#if editing}
    <form
      class="editor"
      bind:this={input}
      onsubmit={(event) => {
        event.preventDefault();
        void save();
      }}
    >
      {#if custom}<Field
          label={m.ai_endpoint_field()}
          placeholder="http://localhost:11434/v1"
          type="url"
          autocomplete="off"
          spellcheck="false"
          bind:value={address}
          onkeydown={escape}
        />{/if}
      {#if custom}<div class="presets" role="group" aria-label={m.connections_presets()}>
          {#each compatiblePresets as preset (preset.base)}<button
              type="button"
              class="preset"
              aria-pressed={address.trim() === preset.base}
              title={preset.models.join(", ")}
              onclick={() => (address = preset.base)}>{preset.label}</button
            >{/each}
        </div>{/if}
      <Field
        label={custom ? m.ai_endpoint_key() : m.ai_key_field({ provider: name })}
        labelHidden={!custom}
        placeholder={custom ? "" : m.ai_key_field({ provider: name })}
        type="password"
        autocomplete="off"
        spellcheck="false"
        bind:value={secret}
        onkeydown={escape}
      />
      <div class="buttons">
        <Button size="compact" variant="ghost" onclick={cancel}>{m.ai_key_cancel()}</Button>
        <Button
          size="compact"
          variant="primary"
          type="submit"
          pending={saving}
          disabled={custom ? !address.trim() : !secret.trim()}>{m.ai_key_save()}</Button
        >
      </div>
    </form>
  {/if}
  {#if message}<p
      class="message"
      data-tone={message.tone}
      role={message.tone === "error" ? "alert" : "status"}
    >
      {message.text}
    </p>{/if}
</div>

<style>
  /* Row metrics match SettingsRow exactly; this row carries a mark and can
     open into its own small form. */
  .ai-row {
    position: relative;
    box-sizing: border-box;
    padding: 12px 18px 12px 14px;
  }

  :global(.ai-row) + .ai-row::before {
    content: "";
    position: absolute;
    inset-inline: 58px 0;
    top: 0;
    height: 1px;
    background: var(--color-border);
  }

  .head {
    display: flex;
    align-items: center;
    gap: 12px;
    min-height: calc(var(--row-page) - 24px);
  }

  .tile {
    display: grid;
    flex: none;
    inline-size: 32px;
    block-size: 32px;
    border-radius: var(--radius-control);
    background: var(--color-fill);
    color: var(--color-text);
    place-items: center;
  }

  .copy {
    flex: 1;
    min-width: 0;
  }

  h3 {
    margin: 0;
    color: var(--color-text);
    font-size: var(--text-page-title);
    font-weight: 500;
    line-height: 19px;
    letter-spacing: -0.008em;
  }

  p {
    margin: 2px 0 0;
    overflow: hidden;
    color: var(--color-muted);
    font-size: var(--text-label);
    line-height: 1.5;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .address {
    font-family: var(--font-mono);
    font-size: var(--text-caption);
  }

  .status {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }

  .dot {
    inline-size: 6px;
    block-size: 6px;
    border-radius: var(--radius-capsule);
    background: var(--color-faint);
  }

  .status[data-tone="valid"] .dot {
    background: var(--color-success);
  }

  .status[data-tone="invalid"] {
    color: var(--color-danger);
  }

  .status[data-tone="invalid"] .dot {
    background: var(--color-danger);
  }

  .actions {
    display: flex;
    flex: none;
    align-items: center;
    gap: 4px;
  }

  :global(.ai-row-more) {
    display: grid;
    inline-size: 28px;
    block-size: 28px;
    padding: 0;
    border: 0;
    border-radius: var(--radius-control-compact);
    background: transparent;
    color: var(--color-muted);
    place-items: center;
    transition: background-color var(--motion-instant) var(--ease-smooth);
  }

  :global(.ai-row-more:hover),
  :global(.ai-row-more[data-state="open"]) {
    background: var(--color-fill-hover);
    color: var(--color-text);
  }

  :global(.ai-row-more:focus-visible) {
    outline: 2px solid var(--color-ring);
    outline-offset: 1px;
  }

  .editor {
    display: grid;
    gap: 10px;
    margin: 12px 0 2px 44px;
    animation: editor-in var(--motion-base) var(--ease-out);
  }

  @keyframes editor-in {
    from {
      opacity: 0;
      translate: 0 -4px;
    }
  }

  .presets {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }

  .preset {
    padding: 4px 8px;
    border: 1px solid var(--color-border);
    border-radius: var(--radius-control-compact);
    background: var(--color-fill);
    color: var(--color-muted);
    cursor: pointer;
    font-size: var(--text-caption);
  }

  .preset[aria-pressed="true"] {
    border-color: var(--color-accent);
    color: var(--color-text);
  }

  .buttons {
    display: flex;
    justify-content: flex-end;
    gap: 6px;
  }

  .message {
    margin: 8px 0 0 44px;
    white-space: normal;
  }

  .local {
    display: flex;
    gap: 6px;
    margin: 10px 0 0 44px;
  }

  a {
    color: var(--color-text);
    text-decoration: underline;
  }

  .message[data-tone="error"] {
    color: var(--color-danger);
  }

  @media (prefers-reduced-motion: reduce) {
    .editor {
      animation: none;
    }
  }
</style>
