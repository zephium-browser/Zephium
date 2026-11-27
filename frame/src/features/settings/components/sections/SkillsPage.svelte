<script lang="ts">
  import { tick } from "svelte";
  import * as m from "$shared/i18n/messages";
  import { reducedMotion } from "$shared/lib/motion";
  import { tabs } from "$domain/tabs";
  import { WorkSkillsSession } from "$domain/work-context";
  import type { WorkSkillFaultV1, WorkSkillRowV1 } from "$shared/ipc/bindings";
  import Button from "$shared/ui/Button";
  import Field from "$shared/ui/Field";
  import Icon from "$shared/ui/Icon";
  import Select from "$shared/ui/Select";
  import SettingsGroup from "$shared/ui/SettingsGroup";
  import Switch from "$shared/ui/Switch";
  import Add01Icon from "@hugeicons/core-free-icons/Add01Icon";
  import {
    parseSkill,
    renderSkill,
    skillName,
    skillTitle,
    type SkillFields,
  } from "../../lib/work-context";

  const profile = $derived(tabs.profile());
  let session = $state.raw<WorkSkillsSession | null>(null);
  $effect(() => {
    const id = profile && profile.kind !== "incognito" ? profile.id : null;
    if (!id) return;
    const owner = new WorkSkillsSession(id);
    session = owner;
    void owner.start();
    return () => owner.dispose();
  });

  const FAULTS: Record<WorkSkillFaultV1, () => string> = {
    no_frontmatter: m.settings_skills_fault_text,
    name: m.settings_skills_fault_name,
    description: m.settings_skills_fault_description,
    role: m.settings_skills_fault_text,
    too_large: m.settings_skills_fault_large,
    empty: m.settings_skills_fault_empty,
    not_found: m.settings_skills_fault_gone,
    built_in: m.settings_skills_fault_builtin,
    taken: m.settings_skills_fault_taken,
    full: m.settings_skills_fault_full,
  };
  const ROLES = [
    { value: "", label: m.settings_skills_role_auto() },
    { value: "lead", label: m.settings_skills_role_lead() },
    { value: "page", label: m.settings_skills_role_page() },
    { value: "light", label: m.settings_skills_role_light() },
  ];

  /**
   * The skill open below the list: a built-in to read, or the person's to
   * write. `previous` is the name it was saved under; null for a new one.
   */
  let open = $state<{
    key: string;
    previous: string | null;
    builtin: boolean;
    fields: SkillFields;
  } | null>(null);
  let title = $state("");
  let editor = $state<HTMLElement>();
  const name = $derived(skillName(title));

  async function show(skill: WorkSkillRowV1) {
    if (open?.previous === skill.name) {
      open = null;
      return;
    }
    const text = await session?.text(skill.name);
    if (text == null) return;
    const fields = parseSkill(text);
    open = {
      key: `${skill.name}:${Date.now()}`,
      previous: skill.name,
      builtin: skill.builtin,
      fields,
    };
    title = skillTitle(fields.name);
    await reveal();
  }
  async function create(from?: SkillFields) {
    const fields: SkillFields = from
      ? { ...from, name: `${from.name}-copy` }
      : { name: "", description: "", tools: "", role: "", body: "" };
    open = { key: `new:${Date.now()}`, previous: null, builtin: false, fields };
    title = from ? skillTitle(fields.name) : "";
    await reveal();
  }
  /** A built-in turned into the person's own version, under the same name. */
  function customize() {
    if (!open) return;
    open = { ...open, key: `mine:${Date.now()}`, builtin: false };
  }
  async function reveal() {
    await tick();
    editor?.scrollIntoView({ block: "nearest", behavior: reducedMotion() ? "instant" : "smooth" });
    editor?.querySelector<HTMLInputElement>("input")?.focus();
  }
  async function save() {
    if (!open || !session) return;
    const text = renderSkill({ ...open.fields, name });
    const saved = await session.change({ kind: "save", previous: open.previous, text });
    if (saved) open = null;
  }
  async function remove() {
    if (!open?.previous || !session) return;
    if (await session.change({ kind: "delete", name: open.previous })) open = null;
  }
  const mine = $derived(
    !!open?.previous && session?.skills.find((skill) => skill.name === open?.previous),
  );
  const tag = (skill: WorkSkillRowV1) =>
    skill.builtin
      ? m.settings_skills_builtin()
      : skill.customized
        ? m.settings_skills_customized()
        : m.settings_skills_yours();
</script>

{#if !profile || profile.kind === "incognito"}
  <p class="skills-note">{m.work_regular_profile()}</p>
{:else}
  <SettingsGroup title={m.settings_skills_title()} description={m.settings_skills_help()}>
    {#each session?.skills ?? [] as skill (skill.name)}
      <div class="skill" class:open={open?.previous === skill.name} class:off={!skill.enabled}>
        <button
          type="button"
          class="words"
          aria-expanded={open?.previous === skill.name}
          onclick={() => void show(skill)}
        >
          <span class="head"
            ><strong>{skillTitle(skill.name)}</strong><span class="tag">{tag(skill)}</span></span
          >
          <span class="description">{skill.description}</span>
        </button>
        <Switch
          label={m.settings_skills_use({ name: skillTitle(skill.name) })}
          labelHidden
          checked={skill.enabled}
          disabled={session?.busy}
          onchange={(on) =>
            void session?.change({ kind: "set_enabled", name: skill.name, enabled: on })}
        />
      </div>
    {/each}
    {#if session?.loaded && session.skills.length === 0}<p class="empty">
        {m.settings_skills_empty()}
      </p>{/if}
    <button type="button" class="add" onclick={() => void create()}>
      <span class="mark" aria-hidden="true"><Icon icon={Add01Icon} size={15} /></span>
      <span>{m.settings_skills_new()}</span>
    </button>
  </SettingsGroup>
  {#if session?.unavailable}<p class="skills-note" role="alert">
      {m.settings_skills_unavailable()}
    </p>{/if}

  {#if open}
    <section class="editor" bind:this={editor} aria-label={m.settings_skills_editor()}>
      <header>
        <h2>
          {open.previous
            ? open.builtin
              ? skillTitle(open.fields.name)
              : m.settings_skills_editing({ name: skillTitle(open.fields.name) })
            : m.settings_skills_new()}
        </h2>
        {#if open.builtin}<p>{m.settings_skills_builtin_help()}</p>{/if}
      </header>
      {#if open.builtin}
        <div class="sheet">
          <p class="when">{open.fields.description}</p>
          <pre>{open.fields.body}</pre>
        </div>
        <footer>
          <Button onclick={() => open && void create(open.fields)}
            >{m.settings_skills_duplicate()}</Button
          >
          <Button variant="primary" onclick={customize}>{m.settings_skills_customize()}</Button>
        </footer>
      {:else}
        {#key open.key}
          <div class="sheet form">
            <Field
              label={m.settings_skills_name()}
              bind:value={title}
              maxlength={60}
              hint={name ? m.settings_skills_file({ name }) : undefined}
            />
            <Field
              label={m.settings_skills_when()}
              placeholder={m.settings_skills_when_placeholder()}
              bind:value={open.fields.description}
              maxlength={200}
            />
            <div class="body">
              <label class="label" for="work-skill-body">{m.settings_skills_instructions()}</label>
              <textarea
                id="work-skill-body"
                class="page"
                spellcheck="true"
                maxlength={16000}
                placeholder={m.settings_skills_instructions_placeholder()}
                bind:value={open.fields.body}
                style:block-size="{Math.min(
                  520,
                  Math.max(240, 22 + open.fields.body.split('\n').length * 19),
                )}px"></textarea>
            </div>
            <div class="advanced">
              <Select
                label={m.settings_skills_role()}
                options={ROLES}
                bind:value={open.fields.role}
              />
              <Field
                label={m.settings_skills_tools()}
                placeholder="web_search, start_part"
                bind:value={open.fields.tools}
                maxlength={400}
              />
            </div>
          </div>
        {/key}
        {#if session?.fault}<p class="skills-note warn" role="alert">
            {FAULTS[session.fault]()}
          </p>{/if}
        <footer>
          {#if mine && !mine.builtin}<Button
              variant="danger"
              disabled={session?.busy}
              onclick={remove}
              >{mine.customized ? m.settings_skills_restore() : m.settings_skills_delete()}</Button
            >{/if}
          <span class="spacer"></span>
          <Button variant="ghost" onclick={() => (open = null)}>{m.settings_skills_cancel()}</Button
          >
          <Button
            variant="primary"
            pending={session?.busy}
            disabled={!name || !open.fields.description.trim() || !open.fields.body.trim()}
            onclick={save}>{m.settings_skills_save()}</Button
          >
        </footer>
      {/if}
    </section>
  {/if}
{/if}

<style>
  .skill {
    position: relative;
    display: flex;
    align-items: center;
    gap: 12px;
    box-sizing: border-box;
    min-block-size: var(--row-page);
    padding: 6px 18px 6px 6px;
  }

  .skill + .skill::before,
  .add::before {
    content: "";
    position: absolute;
    inset-inline: 18px 0;
    inset-block-start: 0;
    block-size: 1px;
    background: var(--color-border);
  }

  .words {
    display: flex;
    flex: 1;
    flex-direction: column;
    align-items: flex-start;
    gap: 2px;
    min-inline-size: 0;
    padding: 8px 12px;
    border: 0;
    border-radius: var(--radius-row);
    background: transparent;
    color: inherit;
    font: inherit;
    text-align: start;
    cursor: default;
    transition: background-color var(--motion-fast) var(--ease-out);
  }

  .words:hover,
  .open .words {
    background: var(--row-hover);
  }

  .words:focus-visible {
    outline: 2px solid var(--color-ring);
    outline-offset: -2px;
  }

  .head {
    display: flex;
    align-items: baseline;
    gap: 8px;
  }

  strong {
    color: var(--color-text);
    font-size: var(--text-page-title);
    font-weight: 500;
    line-height: 19px;
  }

  .tag {
    color: var(--color-faint);
    font-size: var(--text-caption);
  }

  .description {
    color: var(--color-muted);
    font-size: var(--text-label);
    line-height: 16px;
  }

  .off strong,
  .off .description {
    color: var(--color-faint);
  }

  .add {
    position: relative;
    display: flex;
    align-items: center;
    gap: 12px;
    inline-size: 100%;
    min-block-size: 48px;
    padding: 10px 18px;
    border: 0;
    background: transparent;
    color: var(--color-muted);
    font: inherit;
    font-size: var(--text-page-title);
    text-align: start;
    cursor: default;
  }

  .add:hover {
    color: var(--color-text);
  }

  .add:focus-visible {
    outline: 2px solid var(--color-ring);
    outline-offset: -2px;
  }

  .mark {
    display: grid;
    flex: none;
    place-items: center;
    inline-size: 20px;
    block-size: 20px;
  }

  .empty {
    margin: 0;
    padding: 16px 18px 12px;
    color: var(--color-muted);
    font-size: var(--text-body);
  }

  .editor {
    margin-block: 0 34px;
    scroll-margin-block: 24px;
  }

  .editor header {
    padding-inline: 16px;
    margin-block-end: 10px;
  }

  h2 {
    margin: 0;
    color: var(--color-text);
    font-size: 15px;
    font-weight: 600;
    line-height: 20px;
    letter-spacing: -0.012em;
  }

  .editor header p {
    margin: 2px 0 0;
    color: var(--color-muted);
    font-size: var(--text-label);
    line-height: 1.5;
  }

  .sheet {
    display: flex;
    flex-direction: column;
    gap: 12px;
    padding: 18px;
    border-radius: var(--radius-card);
    background: var(--color-surface);
  }

  .when {
    margin: 0;
    color: var(--color-muted);
    font-size: var(--text-body);
  }

  pre {
    max-block-size: 360px;
    margin: 0;
    overflow: auto;
    color: var(--color-text);
    font-family: var(--font-mono);
    font-size: var(--text-label);
    line-height: 1.6;
    white-space: pre-wrap;
  }

  .body {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .label {
    color: var(--color-text);
    font-size: var(--text-label);
    font-weight: 500;
  }

  /* The instructions are a page of their own, set in the mono face they are written in. */
  .page {
    box-sizing: border-box;
    inline-size: 100%;
    padding: 12px 14px;
    border: 0;
    border-radius: var(--radius-row);
    background: var(--color-field);
    color: var(--color-text);
    font-family: var(--font-mono);
    font-size: var(--text-label);
    line-height: 19px;
    resize: vertical;
    outline: none;
    tab-size: 2;
  }

  .page::placeholder {
    color: var(--color-faint);
  }

  .page:focus-visible {
    box-shadow: var(--shadow-field-focus);
  }

  .advanced {
    display: grid;
    grid-template-columns: 200px 1fr;
    gap: 12px;
  }

  footer {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 8px;
    margin-block-start: 12px;
  }

  .spacer {
    flex: 1;
  }

  .skills-note {
    margin: -26px 16px 28px;
    color: var(--color-muted);
    font-size: var(--text-label);
    line-height: 1.5;
  }

  .skills-note.warn {
    margin: 10px 16px 0;
    color: var(--color-warning);
  }
</style>
