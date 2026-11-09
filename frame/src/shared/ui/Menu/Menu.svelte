<script lang="ts" module>
  import type { IconSvgElement } from "@hugeicons/svelte";
  export type MenuEntry =
    | {
        kind: "item";
        id: string;
        label: string;
        icon?: IconSvgElement;
        hint?: string;
        checked?: boolean;
        danger?: boolean;
        disabled?: boolean;
      }
    | { kind: "separator" }
    | { kind: "heading"; label: string };
</script>

<script lang="ts">
  import { DropdownMenu } from "bits-ui";
  import type { Snippet } from "svelte";
  import Icon from "../Icon/Icon.svelte";
  import Mark from "./Mark.svelte";
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
  } = $props();
  let triggerElement = $state<HTMLButtonElement | null>(null);
  let material = $state<"opaque" | undefined>();
  // A menu that can express a choice keeps the mark column on every row, so
  // the labels do not step sideways as the choice moves.
  let choice = $derived(entries.some((entry) => entry.kind === "item" && "checked" in entry));
</script>

<DropdownMenu.Root
  onOpenChange={(open) => {
    if (open) material = menuMaterialFor(triggerElement);
  }}
>
  <DropdownMenu.Trigger bind:ref={triggerElement} class={triggerClass} aria-label={label}>
    {@render trigger()}
  </DropdownMenu.Trigger>
  <DropdownMenu.Portal>
    <DropdownMenu.Content
      data-menu-material={material}
      class={["ui-menu ui-menu-scroll", contentClass].filter(Boolean).join(" ")}
      {side}
      {align}
      customAnchor={layoutAnchor(triggerElement)}
      sideOffset={6}
      collisionPadding={10}
      onCloseAutoFocus={(event) => {
        if (returnFocus && !returnFocus()) event.preventDefault();
      }}
    >
      {#each entries as entry, index (index)}
        {#if entry.kind === "separator"}
          <DropdownMenu.Separator class="ui-menu-separator" />
        {:else if entry.kind === "heading"}
          <div class="ui-menu-heading" role="presentation">{entry.label}</div>
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
            {#if entry.icon}
              <span class="ui-menu-icon" aria-hidden="true"
                ><Icon icon={entry.icon} size={15} /></span
              >
            {/if}
            {entry.label}
            {#if entry.hint}<span class="ui-menu-hint" aria-hidden="true">{entry.hint}</span>{/if}
            {#if choice}<Mark on={entry.checked === true} />{/if}
          </Item>
        {/if}
      {/each}
    </DropdownMenu.Content>
  </DropdownMenu.Portal>
</DropdownMenu.Root>
