<script lang="ts">
  import PreferenceSelect from "../PreferenceSelect.svelte";
  import * as m from "$shared/i18n/messages";
  import { preferences } from "$domain/preferences";
  import Select from "$shared/ui/Select";
  import Switch from "$shared/ui/Switch";
  import SettingsGroup from "$shared/ui/SettingsGroup";
  import SettingsRow from "$shared/ui/SettingsRow";
  import Icon from "$shared/ui/Icon";
  import { Tick02Icon } from "@hugeicons/core-free-icons";
  import { onMount } from "svelte";
  let material = $state("");
  onMount(() => {
    const update = () => {
      material = document.documentElement.dataset.material ?? "none";
    };
    update();
    const observer = new MutationObserver(update);
    observer.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["data-material"],
    });
    return () => observer.disconnect();
  });
  const schemes = [
    { id: "light", label: m.theme_light },
    { id: "dark", label: m.theme_dark },
    { id: "system", label: m.theme_system },
  ] as const;
  let schemeGroup = $state<HTMLElement>();
  let selectedScheme = $derived(
    Math.max(
      0,
      schemes.findIndex((scheme) => scheme.id === preferences.value("appearance")),
    ),
  );
  const chooseScheme = (id: string) => void preferences.set("appearance", id);
  // A radio group moves and selects with the arrow keys, in reading order.
  function stepScheme(event: KeyboardEvent, index: number) {
    const forward = getComputedStyle(event.currentTarget as Element).direction === "rtl" ? -1 : 1;
    const steps: Record<string, number> = {
      ArrowRight: forward,
      ArrowDown: 1,
      ArrowLeft: -forward,
      ArrowUp: -1,
    };
    const step = steps[event.key];
    if (!step || !schemeGroup) return;
    event.preventDefault();
    const next = (index + step + schemes.length) % schemes.length;
    const scheme = schemes[next];
    if (!scheme) return;
    schemeGroup.querySelectorAll<HTMLElement>('[role="radio"]')[next]?.focus();
    chooseScheme(scheme.id);
  }
  const tints = [
    { id: "graphite", label: m.settings_graphite },
    { id: "sky", label: m.settings_sky },
    { id: "sage", label: m.settings_sage },
    { id: "rose", label: m.settings_rose },
    { id: "amber", label: m.settings_amber },
    { id: "teal", label: m.settings_teal },
    { id: "lavender", label: m.settings_lavender },
    { id: "orchid", label: m.settings_orchid },
  ];
  const materials: Record<string, () => string> = {
    liquid_glass: m.settings_material_glass,
    vibrancy: m.settings_material_vibrancy,
    acrylic: m.settings_material_acrylic,
    mica: m.settings_material_mica,
    none: m.settings_material_solid,
  };
</script>

<SettingsGroup title={m.settings_theme()}>
  <div
    bind:this={schemeGroup}
    class="appearance-previews"
    role="radiogroup"
    aria-label={m.settings_color_scheme()}
    data-setting="appearance.theme"
  >
    {#each schemes as scheme, index (scheme.id)}
      <button
        type="button"
        role="radio"
        class="appearance-preview"
        data-preview-theme={scheme.id}
        aria-checked={preferences.value("appearance") === scheme.id}
        tabindex={index === selectedScheme ? 0 : -1}
        onclick={() => chooseScheme(scheme.id)}
        onkeydown={(event) => stepScheme(event, index)}
      >
        <div class="mini-window">
          <div class="mini-sidebar"><span></span><i></i><i></i><i></i></div>
          <div class="mini-page"><span></span><i></i><i></i></div>
        </div>
        <span class="appearance-preview-label">{scheme.label()}</span>
      </button>
    {/each}
  </div>
  <SettingsRow
    settingId="appearance.accent"
    title={m.settings_accent()}
    description={m.settings_accent_desc()}
  >
    <div class="accent-choices" role="group" aria-label={m.settings_accent()}>
      {#each tints as tint (tint.id)}<button
          class="accent-swatch"
          style:--swatch={`var(--color-tint-${tint.id})`}
          aria-label={tint.label()}
          title={tint.label()}
          aria-pressed={preferences.value("ui.accent") === tint.id}
          disabled={preferences.saving()}
          onclick={() => void preferences.set("ui.accent", tint.id)}
          >{#if preferences.value("ui.accent") === tint.id}<Icon
              icon={Tick02Icon}
              size={14}
            />{/if}</button
        >{/each}
    </div>
  </SettingsRow>
</SettingsGroup>
<SettingsGroup title={m.settings_layout()}>
  <SettingsRow
    settingId="appearance.sidebar"
    title={m.settings_sidebar()}
    description={m.settings_sidebar_desc()}
  >
    <Select
      label={m.settings_sidebar()}
      labelHidden
      value={preferences.value("sidebar.mode")}
      disabled={preferences.saving()}
      options={[
        { value: "default", label: m.settings_expanded() },
        { value: "compact", label: m.settings_compact() },
      ]}
      onchange={(v) => void preferences.set("sidebar.mode", v)}
    />
  </SettingsRow>
  <SettingsRow
    settingId="appearance.material"
    title={m.settings_material()}
    description={m.settings_material_desc()}
    ><span class="settings-value">{materials[material]?.() ?? m.settings_material_solid()}</span
    ></SettingsRow
  >
  <PreferenceSelect id="appearance.density" preference="ui.density" />
</SettingsGroup>
<SettingsGroup title={m.settings_motion()}>
  <SettingsRow
    settingId="appearance.motion"
    title={m.settings_reduce_motion()}
    description={m.settings_reduce_motion_desc()}
    ><Switch
      label={m.settings_reduce_motion()}
      labelHidden
      checked={preferences.value("ui.reduce-motion") === "true"}
      disabled={preferences.saving()}
      onchange={(v) => void preferences.set("ui.reduce-motion", String(v))}
    /></SettingsRow
  >
</SettingsGroup>
