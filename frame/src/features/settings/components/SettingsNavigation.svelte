<script lang="ts">
  import * as m from "$shared/i18n/messages";
  import { surface as browser } from "$domain/surface";
  import * as settings from "../lib/settings-state.svelte";
  import { groups, shownSections } from "../lib/settings-model";
  import Icon from "$shared/ui/Icon";
  import IconButton from "$shared/ui/IconButton";
  import { Cancel01Icon, Search01Icon } from "@hugeicons/core-free-icons";
  import { untrack } from "svelte";
  import { captureFrom, playTo, type Origin } from "$shared/lib/plate-glide";
  import { IS_MAC } from "$shared/platform";
  import { fullscreen } from "$shared/lib/fullscreen.svelte";

  // The account mirror stays out of the browser's startup graph.
  const identity = import("./SettingsIdentity.svelte");

  // The current page's plate travels to the next one, as a tab's does.
  let nav = $state<HTMLElement>();
  let current = $derived(settings.query() ? null : settings.section());
  let shown: string | null | undefined;
  let origin: Origin | null = null;
  const plate = () => nav?.querySelector<HTMLElement>('[data-plate][aria-current="page"]') ?? null;
  $effect.pre(() => {
    const next = current;
    untrack(() => {
      if (shown !== undefined && next !== shown) origin = captureFrom(plate());
    });
  });
  $effect(() => {
    const next = current;
    untrack(() => {
      if (shown !== undefined && next !== shown) playTo(origin, plate());
      origin = null;
      shown = next;
    });
  });
</script>

<nav
  bind:this={nav}
  class="settings-sidebar-navigation"
  data-glide-host
  aria-label={m.settings_title()}
>
  <!-- Its name and the way out share the window's own row, beside the lights
       where the system draws them. -->
  <header class="settings-panel-header" data-lights={(IS_MAC && !fullscreen()) || undefined}>
    <h2>{m.settings_title()}</h2>
    <IconButton
      icon={Cancel01Icon}
      label={m.settings_back()}
      onclick={() => void browser.close()}
    />
  </header>
  {#await identity then { default: SettingsIdentity }}<SettingsIdentity />{/await}
  <div class="settings-sidebar-search">
    <Icon icon={Search01Icon} size={14} /><input
      type="search"
      aria-label={m.settings_search()}
      placeholder={m.settings_search()}
      value={settings.query()}
      oninput={(e) => settings.setQuery(e.currentTarget.value)}
    />
  </div>
  <div class="settings-navigation-list" data-glide-scroller>
    {#each groups as group (group.id)}<div class="settings-nav-group">
        <h2>{group.label()}</h2>
        {#each shownSections().filter((item) => item.group === group.id) as item (item.id)}<button
            type="button"
            class="settings-nav-item"
            data-plate
            aria-current={settings.section() === item.id && !settings.query() ? "page" : undefined}
            onclick={() => settings.select(item.id)}
            ><Icon icon={item.icon} size={16} /><span>{item.title()}</span></button
          >{/each}
      </div>{/each}
  </div>
</nav>
