<script lang="ts">
  import type { HTMLInputAttributes } from "svelte/elements";
  import { Search01Icon } from "@hugeicons/core-free-icons";
  import Icon from "../Icon/Icon.svelte";
  let {
    label,
    size = "chrome",
    placeholder,
    value = $bindable(""),
    ref = $bindable(),
    onsubmit,
    oninput,
    inputProps = {},
  }: {
    inputProps?: Omit<HTMLInputAttributes, "value" | "oninput">;
    oninput?: (value: string) => void;
    label: string;
    ref?: HTMLInputElement;
    size?: "chrome" | "page";
    placeholder?: string;
    value?: string;
    onsubmit?: (value: string) => void;
  } = $props();
  const uid = $props.id();
</script>

<form
  role="search"
  class="ui-search"
  data-size={size}
  onsubmit={(event) => {
    event.preventDefault();
    onsubmit?.(value.trim());
  }}
>
  <label class="sr-only" for={uid}>{label}</label><Icon
    icon={Search01Icon}
    size={size === "page" ? 16 : 14}
  /><input
    id={uid}
    bind:this={ref}
    type="search"
    bind:value
    {placeholder}
    autocomplete="off"
    {...inputProps}
    oninput={(event) => oninput?.(event.currentTarget.value)}
  />
</form>

<style>
  .ui-search {
    display: flex;
    align-items: center;
    gap: 6px;
    box-sizing: border-box;
    min-height: var(--search-height);
    padding: 0 10px;
    border: 0;
    border-radius: var(--radius-field);
    color: var(--color-faint);
    background: var(--color-field);
    transition:
      background-color var(--motion-instant) var(--ease-smooth),
      color var(--motion-instant) var(--ease-smooth),
      box-shadow var(--motion-instant) var(--ease-smooth);
  }

  .ui-search:hover {
    background: var(--color-field-hover);
  }

  .ui-search:focus-within,
  .ui-search:has(input[data-caret-parked]) {
    color: var(--color-muted);
    box-shadow: var(--shadow-field-focus);
  }

  input {
    border: 0;
    outline: none;
    min-width: 0;
    width: 100%;
    height: var(--search-height);
    background: transparent;
    color: var(--color-text);
    font: inherit;
    font-size: var(--text-body);
    /* stylelint-disable-next-line property-no-vendor-prefix */
    -webkit-user-select: text;
    user-select: text;
  }

  input::placeholder {
    color: var(--color-faint);
  }

  input::-webkit-search-decoration,
  input::-webkit-search-cancel-button {
    appearance: none;
  }

  /* The page-size search is the same field, larger. Almost round at 46px
     rather than a capsule: a pill in a column of rounded rectangles is the one
     shape that never belongs to the set. */
  .ui-search[data-size="page"] {
    min-height: 46px;
    padding-inline: 18px;
    gap: 12px;
    border-radius: var(--radius-panel);
  }

  .ui-search[data-size="page"] input {
    height: 46px;
    font-size: var(--text-page-body);
  }

  @media (forced-colors: active) {
    .ui-search {
      border: 1px solid ButtonText;
      box-shadow: none;
    }
  }
</style>
