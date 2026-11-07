<script lang="ts">
  import * as m from "$shared/i18n/messages";
  import { preferences } from "$domain/preferences";
  import { tabs } from "$domain/tabs";
  import Button from "$shared/ui/Button";
  import Field from "$shared/ui/Field";

  const LIMIT = 10;
  let draft = $state("");
  let invalid = $state(false);
  let pages = $derived(preferences.value("tabs.startup-pages").split("\n").filter(Boolean));

  async function save(next: string[]) {
    await preferences.set("tabs.startup-pages", next.join("\n"));
    return !preferences.saveFailed();
  }

  async function add() {
    const page = draft.trim();
    if (!page) return;
    if (pages.includes(page)) {
      draft = "";
      return;
    }
    invalid = !(await save([...pages, page]));
    if (!invalid) draft = "";
  }

  async function useCurrent() {
    const open = tabs
      .tabs()
      .filter((tab) => (tab.content ?? "web") === "web" && /^https?:\/\//iu.test(tab.url ?? ""))
      .map((tab) => tab.url!)
      .filter((url, index, all) => url.length <= 2048 && all.indexOf(url) === index)
      .slice(0, LIMIT);
    if (open.length > 0) invalid = !(await save(open));
  }
</script>

<div class="pages" data-setting="general.startup-pages">
  <div class="copy">
    <h3>{m.pref_general_startup_pages()}</h3>
    <p>{m.pref_general_startup_pages_help()}</p>
  </div>
  <form
    onsubmit={(event) => {
      event.preventDefault();
      void add();
    }}
  >
    <Field
      label={m.field_url()}
      labelHidden
      placeholder={m.field_url()}
      bind:value={draft}
      autocomplete="off"
      inputmode="url"
      spellcheck={false}
      maxlength={2048}
      disabled={preferences.saving()}
      error={invalid ? m.field_invalid_url() : undefined}
      oninput={() => (invalid = false)}
    />
    <Button type="submit" disabled={!draft.trim() || preferences.saving() || pages.length >= LIMIT}>
      {m.action_add()}
    </Button>
  </form>
  {#if pages.length > 0}
    <ul aria-label={m.pref_general_startup_pages()}>
      {#each pages as page (page)}
        <li>
          <span>{page}</span><Button
            variant="ghost"
            disabled={preferences.saving()}
            aria-label={`${m.action_remove()} ${page}`}
            onclick={() =>
              void save(pages.filter((kept) => kept !== page)).then((ok) => (invalid = !ok))}
          >
            {m.action_remove()}
          </Button>
        </li>
      {/each}
    </ul>
  {/if}
  <div class="actions">
    <Button variant="ghost" disabled={preferences.saving()} onclick={() => void useCurrent()}>
      {m.general_startup_pages_current()}
    </Button>
  </div>
</div>

<style>
  .pages {
    display: grid;
    gap: 12px;
    padding: 14px 18px;
    border-block-start: 1px solid var(--color-border);
  }

  h3 {
    margin: 0;
    color: var(--color-text);
    font-size: var(--text-page-title);
    font-weight: 500;
    line-height: 19px;
  }

  p {
    margin: 2px 0 0;
    color: var(--color-muted);
    font-size: var(--text-label);
    line-height: 1.5;
  }

  form {
    display: flex;
    align-items: start;
    gap: 8px;
  }

  form :global(.ui-field) {
    flex: 1;
  }

  ul {
    margin: 0;
    padding: 0;
    list-style: none;
  }

  li {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    min-block-size: 36px;
  }

  span {
    overflow: hidden;
    font-size: var(--text-body);
    text-overflow: ellipsis;
  }

  .actions {
    display: flex;
    justify-content: flex-start;
  }
</style>
