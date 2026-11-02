<script lang="ts">
  import { onMount } from "svelte";
  import * as m from "$shared/i18n/messages";
  import { commands, type AboutInfo } from "$shared/ipc/bindings";
  import { preferences } from "$domain/preferences";
  import { runtime } from "$domain/runtime";
  import { releaseNotesUrl, systemUpdateTarget, updates } from "$domain/updates";
  import { IS_MAC } from "$shared/platform";
  import { keymap } from "$domain/keymap";
  import * as notices from "$session/notice.svelte";
  import SettingsGroup from "$shared/ui/SettingsGroup";
  import SettingsRow from "$shared/ui/SettingsRow";
  import Button from "$shared/ui/Button";
  import ZephiumTile from "$shared/ui/ZephiumTile";
  import AboutLinks from "../about/AboutLinks.svelte";
  import { WEBSITE } from "../../lib/links";
  import PreferenceSwitch from "../PreferenceSwitch.svelte";

  let about = $state<AboutInfo | null>(null);
  let confirming = $state(false);
  let resetting = $state(false);
  let outcome = $state<"done" | "failed" | null>(null);
  let report = $state<string | null>(null);
  let reviewing = $state(false);
  let reading = $state<"copy" | "review" | null>(null);
  let copyOutcome = $state<"failed" | "blocked" | null>(null);

  onMount(() => {
    let live = true;
    void updates.refresh();
    void commands
      .aboutInfo()
      .then((info) => {
        if (live) about = info;
      })
      .catch(() => {});
    return () => {
      live = false;
    };
  });

  let platform = $derived(about ? m.about_platform({ os: about.os, arch: about.arch }) : "");

  let update = $derived(updates.status());
  let updatable = $derived(update.state !== "unavailable");
  let updateLine = $derived.by(() => {
    switch (update.state) {
      case "unavailable":
        return m.update_status_unavailable();
      case "idle":
        return m.pref_about_updates_help();
      case "checking":
        return m.update_status_checking();
      case "upToDate":
        return m.update_status_current();
      case "downloading":
        return m.update_status_downloading();
      case "ready":
        return update.retry_reason ?? m.update_status_ready({ version: update.version });
      case "manualInstall":
        return m.update_status_manual({ version: update.version });
      case "installing":
        return m.update_installing();
      case "failed":
        return m.update_status_failed();
    }
  });
  let security = $derived(
    updatable ? systemUpdateTarget(runtime.status().security_advisories) : null,
  );

  const readReport = () => commands.diagnosticsReport().catch(() => null);

  /** The clipboard is written inside the click, with the text still to come:
   *  WebKit refuses a write that waits for the native read first. */
  async function copyDiagnostics() {
    reading = "copy";
    copyOutcome = null;
    const read = readReport();
    let copied = false;
    try {
      const plain = read.then((text) => {
        if (text === null) throw new Error("unavailable");
        return new Blob([text], { type: "text/plain" });
      });
      await navigator.clipboard.write([new ClipboardItem({ "text/plain": plain })]);
      copied = true;
    } catch {
      // Tried again below, as text, once the report is in hand.
    }
    const text = await read;
    reading = null;
    if (text === null) {
      copyOutcome = "failed";
      return;
    }
    report = text;
    if (!copied) {
      copied = await navigator.clipboard.writeText(text).then(
        () => true,
        () => false,
      );
    }
    if (copied) {
      notices.show(m.about_diagnostics_copied());
    } else {
      copyOutcome = "blocked";
      reviewing = true;
    }
  }

  async function toggleReview() {
    if (reviewing) {
      reviewing = false;
      return;
    }
    reading = "review";
    copyOutcome = null;
    const text = await readReport();
    reading = null;
    if (text === null) {
      copyOutcome = "failed";
      return;
    }
    report = text;
    reviewing = true;
  }

  async function reset() {
    resetting = true;
    outcome = null;
    const settled = await preferences.resetAll();
    const shortcuts = await keymap.reset(null);
    resetting = false;
    confirming = false;
    outcome = settled && shortcuts ? "done" : "failed";
  }
</script>

<header class="settings-about hero">
  <ZephiumTile size={88} />
  <div class="identity">
    <span class="zephium-wordmark" role="img" aria-label="Zephium"></span>
    <p class="meta">
      <span class="version">{about ? m.about_version({ version: about.version }) : ""}</span>
      {#if updatable && about}<Button
          size="compact"
          variant="ghost"
          onclick={() =>
            void commands.browserOpenUrl(releaseNotesUrl(about!.version), true).catch(() => {})}
          >{m.update_release_notes()}</Button
        >{/if}
    </p>
    <p class="platform">{platform}</p>
  </div>
  <AboutLinks />
</header>
<SettingsGroup title={m.about_group_software()}>
  <SettingsRow settingId="about.updates" title={m.pref_about_updates()} description={updateLine}>
    {#if update.state === "ready" || update.state === "installing"}
      <Button
        size="compact"
        variant="primary"
        pending={update.state === "installing" || updates.pendingRelaunch()}
        onclick={() => void updates.relaunch()}>{m.update_relaunch()}</Button
      >
    {:else if update.state === "manualInstall"}
      <Button
        size="compact"
        onclick={() => {
          if (update.state === "manualInstall")
            void commands.browserOpenUrl(releaseNotesUrl(update.version), true).catch(() => {});
        }}>{m.update_install_manual()}</Button
      >
    {:else if update.state === "failed"}
      <div class="actions">
        <Button size="compact" onclick={() => void updates.check()}>{m.update_check_now()}</Button>
        <Button
          size="compact"
          onclick={() => void commands.browserOpenUrl(WEBSITE, true).catch(() => {})}
          >{m.update_download_latest()}</Button
        >
      </div>
    {:else if updatable}
      <Button
        size="compact"
        pending={update.state === "checking" || update.state === "downloading"}
        onclick={() => void updates.check()}>{m.update_check_now()}</Button
      >
    {/if}
  </SettingsRow>
  {#if updatable}<PreferenceSwitch id="updates.auto-check" preference="updates.auto-check" />{/if}
  {#if security}
    <SettingsRow
      title={security === "browser_runtime"
        ? m.update_webview_title()
        : IS_MAC
          ? m.update_security_mac_title()
          : m.update_security_system_title()}
      description={security === "browser_runtime"
        ? m.update_webview_detail()
        : m.update_security_detail()}
    >
      {#if security === "operating_system" && IS_MAC}<Button
          size="compact"
          onclick={() => void commands.openSoftwareUpdate().catch(() => {})}
          >{m.update_security_open()}</Button
        >{/if}
    </SettingsRow>
  {/if}
  <SettingsRow
    settingId="about.diagnostics"
    title={m.pref_about_diagnostics()}
    description={copyOutcome === "failed"
      ? m.about_diagnostics_failed()
      : copyOutcome === "blocked"
        ? m.about_diagnostics_blocked()
        : m.pref_about_diagnostics_help()}
  >
    <div class="actions">
      <Button
        size="compact"
        variant="ghost"
        pending={reading === "review"}
        disabled={reading === "copy"}
        aria-expanded={reviewing}
        aria-controls="about-diagnostics-report"
        onclick={() => void toggleReview()}
        >{reviewing ? m.about_diagnostics_hide() : m.about_diagnostics_review()}</Button
      >
      <Button
        size="compact"
        pending={reading === "copy"}
        disabled={reading === "review"}
        onclick={() => void copyDiagnostics()}>{m.about_diagnostics_copy()}</Button
      >
    </div>
  </SettingsRow>
  {#if reviewing && report !== null}
    <div class="report" id="about-diagnostics-report">
      <textarea readonly rows="12" spellcheck="false" aria-label={m.pref_about_diagnostics()}
        >{report}</textarea
      >
    </div>
  {/if}
  <SettingsRow title={m.about_logs()} description={m.about_logs_help()}>
    <Button size="compact" onclick={() => void commands.diagnosticsShowLogs().catch(() => false)}
      >{m.about_logs_show()}</Button
    >
  </SettingsRow>
</SettingsGroup>
<SettingsGroup title={m.settings_advanced()}>
  <SettingsRow
    settingId="about.reset"
    title={m.pref_about_reset()}
    description={outcome === "done"
      ? m.about_reset_done()
      : outcome === "failed"
        ? m.about_reset_failed()
        : m.pref_about_reset_help()}
  >
    <div class="actions">
      {#if confirming}
        <Button
          size="compact"
          variant="ghost"
          disabled={resetting}
          onclick={() => (confirming = false)}>{m.action_cancel()}</Button
        >
        <Button size="compact" variant="danger" pending={resetting} onclick={() => void reset()}
          >{m.about_reset_confirm()}</Button
        >
      {:else}
        <Button
          size="compact"
          onclick={() => {
            outcome = null;
            confirming = true;
          }}>{m.about_reset_action()}</Button
        >
      {/if}
    </div>
  </SettingsRow>
</SettingsGroup>

<style>
  .hero {
    gap: 20px;
    padding-block: 18px 40px;
  }

  .identity {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 6px;
  }

  .zephium-wordmark {
    inline-size: 150px;
    block-size: 22px;
  }

  .meta,
  .platform {
    display: flex;
    align-items: center;
    gap: 4px;
    margin: 0;
    color: var(--color-muted);
    font-size: var(--text-label);
    line-height: 1.4;
  }

  .version {
    color: var(--color-label-secondary);
    font-weight: 500;
  }

  .actions {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: flex-end;
    gap: 6px;
  }

  .report {
    padding: 0 18px 14px;
  }

  .report + :global(.ui-settings-row)::before {
    content: "";
    position: absolute;
    inset-inline: 16px 0;
    top: 0;
    height: 1px;
    background: var(--color-border);
  }

  .report textarea {
    box-sizing: border-box;
    inline-size: 100%;
    margin: 0;
    padding: 12px 14px;
    resize: vertical;
    border: 0;
    border-radius: var(--radius-field);
    background: var(--color-field);
    color: var(--color-text);
    font-family: var(--font-mono);
    font-size: var(--text-label);
    line-height: 1.55;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }

  .report textarea:focus-visible {
    outline: 2px solid var(--color-ring);
    outline-offset: 2px;
  }
</style>
