<script lang="ts">
  import { ArrowRight01Icon } from "@hugeicons/core-free-icons";
  import { surface as browserPage } from "$domain/surface";
  import * as m from "$shared/i18n/messages";
  import Icon from "$shared/ui/Icon";
</script>

<!-- The name of the row and the way on to the page that manages them: the
     whole of the button, and none of the buttons beside it. -->
<button
  type="button"
  data-dismiss
  class="manage"
  aria-label={m.webext_manage()}
  title={m.webext_manage()}
  onclick={() => void browserPage.open("extensions")}
>
  <span class="name">{m.webext_page_title()}</span>
  <span class="chevron" aria-hidden="true"><Icon icon={ArrowRight01Icon} size={14} /></span>
</button>

<style>
  .manage {
    display: flex;
    flex: none;
    align-items: center;
    gap: 2px;
    block-size: 28px;
    padding-inline: var(--menu-item-inset) 6px;
    border: 0;
    border-radius: var(--radius-inset);
    background: transparent;
    color: var(--color-text);
    cursor: default;
    font: inherit;
    font-size: var(--text-body);
    font-weight: 450;
    line-height: 18px;
    outline: none;
    transition: background-color var(--motion-instant) var(--ease-smooth);
  }

  .manage:hover,
  .manage:focus-visible {
    background: var(--row-active);
  }

  .manage:focus-visible {
    outline: 2px solid var(--color-ring);
    outline-offset: -2px;
  }

  .manage:active {
    background: var(--row-pressed);
  }

  .chevron {
    display: grid;
    color: var(--color-faint);
    transition: color var(--motion-instant) var(--ease-smooth);
  }

  .manage:is(:hover, :focus-visible) .chevron {
    color: var(--color-text);
  }
</style>
