<script lang="ts">
  import { preferences, type PreferenceKey } from "$domain/preferences";
  import { fields, type FieldId } from "../lib/catalog";
  import SettingsRow from "$shared/ui/SettingsRow";
  import Switch from "$shared/ui/Switch";

  /** `id` names the row (labels, search); `preference` is what it changes. */
  let {
    id,
    preference,
    description,
  }: { id: FieldId; preference: PreferenceKey; description?: string } = $props();
  let field = $derived(fields[id]);
</script>

<SettingsRow settingId={id} title={field.label()} description={description ?? field.description()}
  ><Switch
    label={field.label()}
    labelHidden
    checked={preferences.value(preference) === "true"}
    disabled={preferences.saving()}
    onchange={(value) => void preferences.set(preference, String(value))}
  /></SettingsRow
>
