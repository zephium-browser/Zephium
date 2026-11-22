<script lang="ts">
  import { preferences } from "$domain/preferences";
  import * as m from "$shared/i18n/messages";
  import SettingsGroup from "$shared/ui/SettingsGroup";
  import SettingsRow from "$shared/ui/SettingsRow";
  import Switch from "$shared/ui/Switch";

  let signInPrompts = $derived(preferences.showSignInPrompts());
  let modeSwitch = $derived(preferences.value("ui.mode-switch") !== "false");

  async function save(key: "ui.sign-in-prompts" | "ui.mode-switch", next: boolean) {
    // Track the control's pending choice so a rejected write can restore its
    // previous value without remounting the focused switch.
    if (key === "ui.sign-in-prompts") signInPrompts = next;
    else modeSwitch = next;
    await preferences.set(key, String(next));
    signInPrompts = preferences.showSignInPrompts();
    modeSwitch = preferences.value("ui.mode-switch") !== "false";
  }
</script>

<SettingsGroup title={m.look_browser_controls()}>
  <SettingsRow
    settingId="appearance.sign-in-prompts"
    title={m.look_sign_in_prompts()}
    description={m.look_sign_in_prompts_help()}
  >
    <Switch
      label={m.look_sign_in_prompts()}
      labelHidden
      checked={signInPrompts}
      disabled={preferences.saving()}
      onchange={(value) => void save("ui.sign-in-prompts", value)}
    />
  </SettingsRow>
  <SettingsRow
    settingId="appearance.mode-switch"
    title={m.look_mode_switch()}
    description={m.look_mode_switch_help()}
  >
    <Switch
      label={m.look_mode_switch()}
      labelHidden
      checked={modeSwitch}
      disabled={preferences.saving()}
      onchange={(value) => void save("ui.mode-switch", value)}
    />
  </SettingsRow>
</SettingsGroup>
