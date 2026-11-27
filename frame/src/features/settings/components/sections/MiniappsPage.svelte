<script lang="ts">
  import * as m from "$shared/i18n/messages";
  import { commands, type MiniAppView } from "$shared/ipc/bindings";
  import SettingsGroup from "$shared/ui/SettingsGroup";
  import Button from "$shared/ui/Button";
  import Field from "$shared/ui/Field";
  import Icon from "$shared/ui/Icon";
  import FavIcon from "$shared/ui/FavIcon";
  import IconButton from "$shared/ui/IconButton";
  import Globe02Icon from "@hugeicons/core-free-icons/Globe02Icon";
  import Cancel01Icon from "@hugeicons/core-free-icons/Cancel01Icon";
  import { onMount } from "svelte";

  let apps = $state<MiniAppView[]>([]);
  let loading = $state(true);
  let address = $state("");
  let busy = $state(false);
  let failure = $state<string | null>(null);
  let recovered = $state(false);
  let invalid = $derived.by(() => {
    const value = address.trim();
    if (!value) return false;
    try {
      return new URL(value).protocol !== "https:";
    } catch {
      return true;
    }
  });

  async function refresh() {
    apps = await commands.miniAppsList().catch(() => []);
    loading = false;
  }

  onMount(() => void refresh());

  async function install() {
    if (busy || !address.trim() || invalid) return;
    busy = true;
    failure = null;
    recovered = false;
    try {
      const result = await commands.miniAppsInstall(address.trim());
      if (result.status !== "ok") {
        failure = result.error;
        return;
      }
      const installed = result.data;
      apps = [...apps.filter((app) => app.id !== installed.id), installed];
      recovered = installed.registry_recovered;
      address = "";
    } catch {
      failure = m.miniapps_failed();
    } finally {
      busy = false;
    }
  }

  async function remove(id: string) {
    if (busy) return;
    busy = true;
    const removed = await commands.miniAppsRemove(id).catch(() => false);
    if (removed) apps = apps.filter((app) => app.id !== id);
    else failure = m.miniapps_failed();
    busy = false;
  }

  function host(value: string): string {
    try {
      return new URL(value).host.replace(/^www\./u, "");
    } catch {
      return value;
    }
  }

  function iconPixels(icon: string | null): ImageData | null {
    if (!icon?.startsWith("rgba32:")) return null;
    try {
      const decoded = atob(icon.slice("rgba32:".length));
      if (decoded.length !== 32 * 32 * 4) return null;
      const pixels = new Uint8ClampedArray(decoded.length);
      for (let index = 0; index < decoded.length; index += 1) {
        pixels[index] = decoded.charCodeAt(index);
      }
      return new ImageData(pixels, 32, 32);
    } catch {
      return null;
    }
  }

  let appIcons = $derived(apps.map((app) => ({ ...app, pixels: iconPixels(app.icon) })));
</script>

<SettingsGroup title={m.miniapps_title()} description={m.miniapps_description()}>
  <form
    class="install"
    onsubmit={(event) => {
      event.preventDefault();
      void install();
    }}
  >
    <Field
      label={m.miniapps_install_label()}
      labelHidden
      value={address}
      maxlength={2048}
      spellcheck={false}
      autocomplete="off"
      placeholder={m.miniapps_install_placeholder()}
      error={invalid ? m.miniapps_invalid() : undefined}
      oninput={(event) => {
        address = event.currentTarget.value;
        failure = null;
        recovered = false;
      }}
    />
    <Button
      type="submit"
      variant="primary"
      disabled={busy || !address.trim() || invalid}
      pending={busy}>{busy ? m.miniapps_installing() : m.miniapps_install()}</Button
    >
  </form>

  {#if loading}
    <p class="note">{m.surface_loading()}</p>
  {:else if apps.length === 0}
    <p class="note">{m.miniapps_empty()}</p>
  {:else}
    <ul class="apps">
      {#each appIcons as app (app.id)}
        <li class="app">
          <span class="mark" aria-hidden="true">
            <Icon icon={Globe02Icon} size={18} />
            <FavIcon image={app.pixels} fallback={Globe02Icon} size={32} />
          </span>
          <span class="words">
            <strong>{app.name}</strong>
            <span>{host(app.start_url)}</span>
          </span>
          <span class="scope">{m.miniapps_scope({ scope: app.scope })}</span>
          <Button
            size="compact"
            onclick={() => void commands.browserOpenUrl(app.start_url, true).catch(() => {})}
            >{m.miniapps_open()}</Button
          >
          <IconButton
            icon={Cancel01Icon}
            size={14}
            buttonSize={26}
            label={m.miniapps_remove()}
            disabled={busy}
            onclick={() => void remove(app.id)}
          />
        </li>
      {/each}
    </ul>
  {/if}

  {#if failure}<p class="note error" role="alert">{failure}</p>{/if}
  {#if recovered}<p class="note" role="status">{m.miniapps_recovered()}</p>{/if}
</SettingsGroup>

<style>
  .install {
    display: flex;
    gap: 8px;
    align-items: start;
    padding: 14px 18px;
  }

  .install :global(.ui-field) {
    flex: 1;
    min-inline-size: 0;
  }

  .apps {
    display: grid;
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .app {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 10px 18px;
    border-top: 1px solid var(--color-border);
  }

  .mark {
    position: relative;
    display: grid;
    flex: none;
    place-items: center;
    inline-size: 30px;
    block-size: 30px;
    border-radius: var(--radius-inset);
    color: var(--color-muted);
    background: var(--color-fill);
  }

  .mark :global([data-favicon]) {
    position: absolute;
    inset: 0;
    inline-size: 100%;
    block-size: 100%;
    border-radius: inherit;
    object-fit: contain;
    background: var(--color-fill);
  }

  .words {
    display: grid;
    flex: 1;
    min-inline-size: 0;
    gap: 1px;
  }

  .words strong,
  .words span {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .words strong {
    font-size: var(--text-page-title);
    font-weight: 500;
  }

  .words span,
  .scope,
  .note {
    font-size: var(--text-label);
    color: var(--color-muted);
  }

  .scope {
    flex: none;
    max-inline-size: 34%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .note {
    margin: 0;
    padding: 12px 18px;
  }

  .note.error {
    color: var(--color-danger);
  }
</style>
