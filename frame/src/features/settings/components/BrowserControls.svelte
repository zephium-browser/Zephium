<script lang="ts">
  import { preferences } from "$domain/preferences";
  import * as m from "$shared/i18n/messages";
  import SettingsGroup from "$shared/ui/SettingsGroup";
  import SettingsRow from "$shared/ui/SettingsRow";
  import Switch from "$shared/ui/Switch";
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
      checked={preferences.showSignInPrompts()}
      disabled={preferences.saving()}
      onchange={(value) => void preferences.set("ui.sign-in-prompts", String(value))}
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
      checked={preferences.value("ui.mode-switch") !== "false"}
      disabled={preferences.saving()}
      onchange={(value) => void preferences.set("ui.mode-switch", String(value))}
    />
  </SettingsRow>
</SettingsGroup>
