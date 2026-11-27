<script lang="ts">
  import { onMount } from "svelte";
  import { CpuIcon, Key01Icon } from "@hugeicons/core-free-icons";
  import type { IconSvgElement } from "@hugeicons/svelte";
  import * as m from "$shared/i18n/messages";
  import { commands, type WorkLocalServerV1 } from "$shared/ipc/bindings";
  import Icon from "$shared/ui/Icon";
  import ZephiumTile from "$shared/ui/ZephiumTile";
  import type { Mind } from "../../lib/minds";

  let {
    mind = $bindable(null),
    server = $bindable(null),
    discovering = $bindable(true),
    onsubmit,
  }: {
    mind?: Mind | null;
    /** A model server already running here, connected if chosen. */
    server?: WorkLocalServerV1 | null;
    discovering?: boolean;
    onsubmit: () => void;
  } = $props();

  // Offered only where hosted models exist; reading this contacts no server.
  let hosted = $state(false);

  onMount(() => {
    let live = true;
    void Promise.resolve()
      .then(() => commands.accountGet())
      .then(({ account }) => {
        if (live) hosted = account.available && account.hosted_models !== false;
      })
      .catch(() => {});
    if (!discovering) discovering = true;
    // Asks only this computer's own ports.
    void Promise.resolve()
      .then(() => commands.workLocalServers())
      .then((found) => {
        if (live) server = found.find((entry) => entry.models > 0) ?? null;
      })
      .catch(() => {})
      .finally(() => {
        if (live) discovering = false;
      });
    return () => (live = false);
  });

  const names = { ollama: "Ollama", lm_studio: "LM Studio" } as const;

  type Option = {
    id: Mind;
    /** Zephium shows its own app icon instead. */
    icon?: IconSvgElement;
    title: string;
    body: string;
    found: string | null;
  };

  const options = $derived.by(() => {
    const list: Option[] = [];
    if (hosted)
      list.push({
        id: "zephium",
        title: m.onb_ai_zephium(),
        body: m.onb_ai_zephium_body(),
        found: null,
      });
    list.push(
      { id: "key", icon: Key01Icon, title: m.onb_ai_key(), body: m.onb_ai_key_body(), found: null },
      {
        id: "local",
        icon: CpuIcon,
        title: m.onb_ai_local(),
        body: m.onb_ai_local_body(),
        found: server ? m.onb_ai_found({ server: names[server.kind] }) : null,
      },
    );
    return list;
  });

  function keydown(event: KeyboardEvent) {
    if (event.key !== "Enter" || event.isComposing) return;
    event.preventDefault();
    onsubmit();
  }
</script>

<div class="options" role="radiogroup" aria-label={m.onb_ai_options()}>
  {#each options as option, index (option.id)}
    <label class="option" style:--i={index}>
      <input
        type="radio"
        name="onboarding-model"
        value={option.id}
        bind:group={mind}
        onkeydown={keydown}
      />
      <span class="top">
        {#if option.icon}<span class="tile"><Icon icon={option.icon} size={18} /></span
          >{:else}<ZephiumTile size={36} />{/if}
        {#if option.found}<span class="found">{option.found}</span>{/if}
      </span>
      <span class="title">{option.title}</span>
      <span class="body">{option.body}</span>
    </label>
  {/each}
</div>

<style>
  .options {
    display: flex;
    gap: 16px;
  }

  .option {
    position: relative;
    display: flex;
    flex-direction: column;
    gap: 6px;
    box-sizing: border-box;
    inline-size: 248px;
    min-block-size: 176px;
    padding: 18px 18px 20px;
    border-radius: var(--radius-card);
    background: var(--color-card);
    box-shadow: var(--shadow-raise);
    cursor: default;
    animation: lift 620ms var(--ease-emphasized) backwards;
    animation-delay: calc(300ms + var(--i) * 50ms);
    transition:
      background-color var(--motion-fast) var(--ease-out),
      box-shadow var(--motion-fast) var(--ease-out),
      scale var(--motion-slow) var(--ease-spring);
  }

  .option:hover {
    background: var(--color-fill-hover);
  }

  .option:active {
    scale: 0.985;
  }

  /* The radio is the control; the card is how it looks. */
  input {
    position: absolute;
    inset: 0;
    margin: 0;
    opacity: 0;
    cursor: default;
  }

  .option:has(input:checked) {
    background: var(--color-fill-active);
    box-shadow: inset 0 0 0 1.5px color-mix(in srgb, var(--color-lit) 75%, transparent);
  }

  .option:has(input:focus-visible) {
    outline: 2px solid var(--color-ring);
    outline-offset: 2px;
  }

  .top {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-block-end: 10px;
  }

  .tile {
    display: grid;
    inline-size: 36px;
    block-size: 36px;
    border-radius: var(--radius-control);
    background: var(--color-fill);
    color: var(--color-text);
    place-items: center;
  }

  .found {
    display: flex;
    align-items: center;
    gap: 6px;
    color: var(--color-muted);
    font-size: 11.5px;
    font-weight: 500;
  }

  .found::before {
    content: "";
    inline-size: 6px;
    block-size: 6px;
    border-radius: 50%;
    background: var(--color-success);
  }

  .title {
    color: var(--color-text);
    font-size: 15px;
    font-weight: 500;
    letter-spacing: -0.01em;
  }

  .body {
    color: var(--color-muted);
    font-size: 13px;
    line-height: 1.45;
  }

  @keyframes lift {
    from {
      opacity: 0;
      transform: translateY(10px);
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .option {
      animation: none;
    }
  }
</style>
