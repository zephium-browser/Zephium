<script lang="ts" module>
  import type { IconSvgElement } from "@hugeicons/svelte";
  export type MenuEntry =
    | {
        kind: "item";
        id: string;
        label: string;
        icon?: IconSvgElement;
        /** A site's own mark, drawn in place of `icon` once it has arrived. */
        site?: { image: ImageData | null; tone?: "dark" | "light" | "mid" };
        /** A mark shipped with the app, drawn in place of `icon`. */
        image?: string;
        hint?: string;
        checked?: boolean;
        danger?: boolean;
        disabled?: boolean;
      }
    | { kind: "separator" }
    | { kind: "heading"; label: string }
    | {
        kind: "submenu";
        label: string;
        icon?: IconSvgElement;
        entries: MenuEntry[];
      };
</script>

<script lang="ts">
  import { DropdownMenu } from "bits-ui";
  import type { Snippet } from "svelte";
  import { ArrowRight01Icon } from "@hugeicons/core-free-icons";
  import Icon from "../Icon/Icon.svelte";
  import FavIcon from "../FavIcon/FavIcon.svelte";
  import Mark from "./Mark.svelte";
  import MenuStep from "./MenuStep.svelte";
  import "./popover.css";
  import { menuMaterialFor } from "./material";
  import { layoutAnchor } from "./anchor";
  let {
    label,
    entries,
    trigger,
    triggerClass = "",
    contentClass = "",
    side = "bottom",
    align = "start",
    returnFocus,
    onselect,
    onopenchange,
    open = $bindable(false),
  }: {
    label: string;
    entries: MenuEntry[];
    trigger: Snippet;
    triggerClass?: string;
    /** Added to the floating panel, for a surface that needs its own material. */
    contentClass?: string;
    side?: "top" | "bottom" | "left" | "right";
    align?: "start" | "center" | "end";
    /** Asked as the menu closes; false leaves focus where the chosen action put
     *  it, such as a field the action opened, instead of back on the trigger. */
    returnFocus?: () => boolean;
    onselect: (id: string) => void;
    onopenchange?: (open: boolean) => void;
    open?: boolean;
  } = $props();
  let triggerElement = $state<HTMLButtonElement | null>(null);
  let material = $state<"opaque" | undefined>();
  // A menu that can express a choice keeps the mark column on every row, so
  // the labels do not step sideways as the choice moves.
  const choosing = (list: MenuEntry[]) =>
    list.some((entry) => entry.kind === "item" && "checked" in entry);
  let contentClasses = $derived(["ui-menu ui-menu-scroll", contentClass].filter(Boolean).join(" "));
</script>

{#snippet lead(entry: Extract<MenuEntry, { kind: "item" | "submenu" }>)}
  {#if "image" in entry && entry.image}
    <span class="ui-menu-icon" aria-hidden="true"
      ><img src={entry.image} width="16" height="16" alt="" draggable="false" /></span
    >
  {:else if "site" in entry && entry.site}
    <span class="ui-menu-icon" aria-hidden="true"
      ><FavIcon
        image={entry.site.image}
        tone={entry.site.tone}
        size={16}
        lit
        fallback={entry.icon}
      /></span
    >
  {:else if entry.icon}
    <span class="ui-menu-icon" aria-hidden="true"><Icon icon={entry.icon} size={15} /></span>
  {/if}
{/snippet}

{#snippet list(items: MenuEntry[])}
  {@const choice = choosing(items)}
  {#each items as entry, index (index)}
    {#if entry.kind === "separator"}
      <DropdownMenu.Separator class="ui-menu-separator" />
    {:else if entry.kind === "heading"}
      <div class="ui-menu-heading" role="presentation">{entry.label}</div>
    {:else if entry.kind === "submenu"}
      <DropdownMenu.Sub>
        <DropdownMenu.SubTrigger class="ui-menu-item">
          {@render lead(entry)}
          {entry.label}
          <span class="ui-menu-chevron" aria-hidden="true"
            ><Icon icon={ArrowRight01Icon} size={12} strokeWidth={2} /></span
          >
        </DropdownMenu.SubTrigger>
        <!-- Out of the parent's scroller, which would clip it to a scrollbar. -->
        <DropdownMenu.Portal>
          <DropdownMenu.SubContent
            data-menu-material={material}
            class={`${contentClasses} ui-menu-sub`}
            sideOffset={4}
            collisionPadding={10}
          >
            <MenuStep dir="up" />
            {@render list(entry.entries)}
            <MenuStep dir="down" />
          </DropdownMenu.SubContent>
        </DropdownMenu.Portal>
      </DropdownMenu.Sub>
    {:else}
      {@const Item = "checked" in entry ? DropdownMenu.CheckboxItem : DropdownMenu.Item}
      <Item
        {..."checked" in entry ? { checked: entry.checked === true } : {}}
        class="ui-menu-item"
        textValue={entry.label}
        disabled={entry.disabled}
        data-danger={entry.danger || undefined}
        onSelect={() => onselect(entry.id)}
      >
        {@render lead(entry)}
        {entry.label}
        {#if entry.hint}<span class="ui-menu-hint" aria-hidden="true">{entry.hint}</span>{/if}
        {#if choice}<Mark on={entry.checked === true} />{/if}
      </Item>
    {/if}
  {/each}
{/snippet}

<DropdownMenu.Root
  bind:open
  onOpenChange={(open) => {
    if (open) material = menuMaterialFor(triggerElement);
    onopenchange?.(open);
  }}
>
  <DropdownMenu.Trigger bind:ref={triggerElement} class={triggerClass} aria-label={label}>
    {@render trigger()}
  </DropdownMenu.Trigger>
  <DropdownMenu.Portal>
    <DropdownMenu.Content
      data-menu-material={material}
      class={contentClasses}
      {side}
      {align}
      customAnchor={layoutAnchor(triggerElement)}
      sideOffset={6}
      collisionPadding={10}
      onCloseAutoFocus={(event) => {
        if (returnFocus && !returnFocus()) event.preventDefault();
      }}
    >
      <MenuStep dir="up" />
      {@render list(entries)}
      <MenuStep dir="down" />
    </DropdownMenu.Content>
  </DropdownMenu.Portal>
</DropdownMenu.Root>
