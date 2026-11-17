<script lang="ts">
  import { onMount } from "svelte";
  import * as m from "$shared/i18n/messages";
  import type { VoiceLanguageV1 } from "$shared/ipc/bindings";
  import { accountSession, planName } from "$domain/account";
  import { keymap } from "$domain/keymap";
  import { TalkSettings, languageName } from "$domain/voice";
  import { voiceSession } from "$domain/voice-session";
  import { acceleratorKeys } from "$shared/lib/accelerator";
  import { IS_MAC } from "$shared/platform";
  import Button from "$shared/ui/Button";
  import KeyHint from "$shared/ui/KeyHint";
  import Select from "$shared/ui/Select";
  import SettingsGroup from "$shared/ui/SettingsGroup";
  import SettingsRow from "$shared/ui/SettingsRow";
  import Switch from "$shared/ui/Switch";
  import * as settings from "../../lib/settings-state.svelte";

  let { talk = new TalkSettings() }: { talk?: TalkSettings } = $props();

  onMount(() => {
    void talk.read();
    void accountSession.retain();
    return () => {
      talk.dispose();
      accountSession.release();
    };
  });

  const stored = $derived(talk.settings);
  const preferences = $derived(stored?.preferences ?? null);
  const view = $derived(voiceSession.view());
  const account = $derived(accountSession.account());
  const locale = () => document.documentElement.lang || "en";

  const keys = $derived.by(() => {
    const accelerator = keymap.all().find((entry) => entry.id === "voice.talk")?.accelerator;
    return accelerator ? acceleratorKeys(accelerator, IS_MAC) : [];
  });

  /** Why voice cannot be used here, in the person's terms; null when it can. */
  const blocked = $derived.by(() => {
    switch (view?.availability) {
      case "ai_off":
        return m.talk_settings_ai_off();
      case "private":
        return m.talk_settings_private();
      case "unsupported":
        return m.talk_settings_unsupported();
      default:
        return null;
    }
  });

  const DEFAULT = "default";
  const voiceName = (id: string) => id.charAt(0).toLocaleUpperCase(locale()) + id.slice(1);
  const voiceOptions = $derived([
    {
      value: DEFAULT,
      label: stored?.default_voice
        ? m.talk_settings_voice_default_named({ voice: voiceName(stored.default_voice) })
        : m.talk_settings_voice_default(),
    },
    ...(stored?.voices ?? []).map((voice) => ({ value: voice, label: voiceName(voice) })),
    ...(preferences?.voice && !stored?.voices.includes(preferences.voice)
      ? [{ value: preferences.voice, label: voiceName(preferences.voice) }]
      : []),
  ]);

  const LANGUAGES: VoiceLanguageV1[] = ["en", "es", "de", "fr", "it", "pt", "pl", "ru", "ja", "zh"];
  const languageOptions = $derived([
    { value: "automatic", label: m.talk_settings_language_auto() },
    ...LANGUAGES.map((tag) => ({ value: tag, label: languageName(tag, locale()) })).sort((a, b) =>
      a.label.localeCompare(b.label, locale()),
    ),
  ]);

  const signedIn = $derived(!!account?.signed_in);
  const planRequired = $derived(view?.fault === "plan_required");
  const plan = $derived(planName(account?.plan));
  const planLine = $derived.by(() => {
    if (!signedIn || view?.availability === "signed_out") return m.talk_settings_plan_signed_out();
    if (planRequired) return m.talk_settings_plan_required();
    return plan ? m.talk_settings_plan_ok_named({ plan }) : m.talk_settings_plan_ok();
  });
</script>

{#if blocked}<p class="voice-note">{blocked}</p>{/if}

<SettingsGroup title={m.talk_settings_talk_group()} description={m.talk_settings_privacy()}>
  <SettingsRow
    settingId="talk.key"
    title={m.talk_settings_key()}
    description={m.talk_settings_key_desc()}
  >
    <div class="voice-key">
      {#if keys.length}<KeyHint {keys} label={keys.join(" ")} />{:else}<span class="voice-unset"
          >{m.talk_settings_key_none()}</span
        >{/if}
      <Button
        size="compact"
        variant="ghost"
        onclick={() => settings.select("shortcuts", "keymap.voice.talk")}
        >{m.talk_settings_key_change()}</Button
      >
    </div>
  </SettingsRow>
  <SettingsRow
    settingId="talk.language"
    title={m.talk_settings_language()}
    description={m.talk_settings_language_desc()}
    >{#key talk.refusals}<Select
        label={m.talk_settings_language()}
        labelHidden
        options={languageOptions}
        value={preferences?.language ?? "automatic"}
        disabled={!preferences || talk.busy}
        onchange={(value) => void talk.change({ language: value as VoiceLanguageV1 })}
      />{/key}</SettingsRow
  >
  <SettingsRow
    settingId="talk.sounds"
    title={m.talk_settings_sounds()}
    description={m.talk_settings_sounds_desc()}
    >{#key talk.refusals}<Switch
        label={m.talk_settings_sounds()}
        labelHidden
        checked={preferences?.earcons ?? false}
        disabled={!preferences || talk.busy}
        onchange={(earcons) => void talk.change({ earcons })}
      />{/key}</SettingsRow
  >
</SettingsGroup>

<SettingsGroup title={m.talk_settings_replies_group()}>
  <SettingsRow
    settingId="talk.spoken"
    title={m.talk_settings_spoken()}
    description={m.talk_settings_spoken_desc()}
    >{#key talk.refusals}<Switch
        label={m.talk_settings_spoken()}
        labelHidden
        checked={preferences?.spoken ?? false}
        disabled={!preferences || talk.busy}
        onchange={(spoken) => void talk.change({ spoken })}
      />{/key}</SettingsRow
  >
  <SettingsRow
    settingId="talk.voice"
    title={m.talk_settings_voice()}
    description={stored && !stored.voices.length
      ? m.talk_settings_voice_pending()
      : m.talk_settings_voice_desc()}
    >{#key talk.refusals}<Select
        label={m.talk_settings_voice()}
        labelHidden
        options={voiceOptions}
        value={preferences?.voice ?? DEFAULT}
        disabled={!preferences || !preferences.spoken || talk.busy}
        onchange={(value) => void talk.change({ voice: value === DEFAULT ? null : value })}
      />{/key}</SettingsRow
  >
</SettingsGroup>
{#if talk.unavailable && !stored}<p class="voice-note" role="alert">
    {m.talk_settings_unavailable()}
  </p>{:else if talk.refused}<p class="voice-note" role="alert">{m.talk_settings_refused()}</p>{/if}

<SettingsGroup title={m.talk_settings_plan_group()}>
  <SettingsRow title={m.talk_settings_plan_title()} description={planLine}>
    <Button
      size="compact"
      variant={signedIn && !planRequired ? "secondary" : "primary"}
      onclick={() => settings.select("account")}
      >{!signedIn
        ? m.talk_settings_sign_in()
        : planRequired
          ? m.talk_fault_see_plans()
          : m.talk_settings_plan_account()}</Button
    >
  </SettingsRow>
</SettingsGroup>

<style>
  .voice-note {
    margin: 0 16px 20px;
    color: var(--color-muted);
    font-size: var(--text-label);
    line-height: 1.5;
  }

  .voice-key {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .voice-unset {
    color: var(--color-faint);
    font-size: var(--text-label);
  }
</style>
