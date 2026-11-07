<script lang="ts">
  import * as m from "$shared/i18n/messages";
  import { tabs } from "$domain/tabs";
  import { favicons } from "$domain/favicons";
  import FavIcon from "$shared/ui/FavIcon";
  import Icon from "$shared/ui/Icon";
  import type { Person } from "../lib/conversation";
  import { suggestionLabel } from "../lib/suggestions";
  import { Globe02Icon, QuoteDownIcon, Target02Icon } from "../lib/icons";
  import "$shared/ui/Shine/shine.css";
  import "./assist.css";

  let { message }: { message: Person } = $props();

  const live = $derived(new Map(tabs.tabs().map((tab) => [tab.id, tab])));
  const words = $derived(
    message.text || (message.suggestion ? suggestionLabel(message.suggestion) : ""),
  );

  function markOf(tab: string, host: string) {
    const known = live.get(tab);
    return known ? favicons.mark(known.icon, known.url) : favicons.forPage(`https://${host}`);
  }
</script>

<div class="person assist-rise" aria-label={m.assist_person_label()}>
  {#if message.attachments.length}
    <ul class="attached" aria-label={m.assist_attached_label()}>
      {#each message.attachments as attachment, index (index)}
        <li
          class="chip"
          title={attachment.kind === "page"
            ? attachment.title
            : attachment.kind === "selection"
              ? attachment.excerpt
              : attachment.label}
        >
          {#if attachment.kind === "page"}
            {@const mark = markOf(attachment.tab, attachment.host)}
            <FavIcon
              image={mark?.image ?? null}
              tone={mark?.tone}
              size={13}
              fallback={Globe02Icon}
            />
            <span>{attachment.host}</span>
          {:else if attachment.kind === "element"}
            <Icon icon={Target02Icon} size={12} strokeWidth={1.8} />
            <span>{attachment.label}</span>
          {:else}
            <Icon icon={QuoteDownIcon} size={12} strokeWidth={1.8} />
            <span>{attachment.excerpt}</span>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}
  {#if words}<p class="bubble ui-shine" data-selectable>{words}</p>{/if}
</div>

<style>
  .person {
    display: flex;
    flex-direction: column;
    align-items: flex-end;
    gap: 6px;
    padding-inline-start: 18%;
  }

  .attached {
    display: flex;
    flex-wrap: wrap;
    justify-content: flex-end;
    gap: 4px;
    max-inline-size: 100%;
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .chip {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    box-sizing: border-box;
    max-inline-size: 100%;
    block-size: 20px;
    padding: 0 7px 0 5px;
    border-radius: var(--radius-inset);
    background: var(--color-fill);
    box-shadow: inset 0 0 0 0.5px var(--color-border);
    color: var(--color-muted);
    font-size: 11.5px;
  }

  .chip span {
    min-inline-size: 0;
    max-inline-size: 180px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .bubble {
    --shine-radius: 18px;

    max-inline-size: 100%;
    box-sizing: border-box;
    margin: 0;
    padding: 8px 13px;
    /* stylelint-disable-next-line property-no-vendor-prefix */
    -webkit-user-select: text;
    user-select: text;
    color: var(--color-text);
    font-size: 13.5px;
    letter-spacing: -0.003em;
    line-height: 20px;
    overflow-wrap: anywhere;
    white-space: pre-wrap;
  }

  @media (forced-colors: active) {
    .bubble {
      border: 1px solid CanvasText;
    }
  }
</style>
