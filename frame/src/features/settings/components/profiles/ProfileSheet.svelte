<script lang="ts">
  import { untrack } from "svelte";
  import { Dialog } from "bits-ui";
  import type { ProfileEntryView } from "$shared/ipc/bindings";
  import { drawn, profiles, type AvatarDraft } from "$domain/profiles";
  import * as m from "$shared/i18n/messages";
  import Button from "$shared/ui/Button";
  import Field from "$shared/ui/Field";
  import AvatarPicker, { type AvatarChoice } from "./AvatarPicker.svelte";

  let {
    open = $bindable(false),
    profile = null,
    current = false,
    color = 1,
    andSwitch = false,
  }: {
    open?: boolean;
    /** The profile edited; none creates one. */
    profile?: ProfileEntryView | null;
    /** Whether this window browses as `profile`, which keeps it from deletion. */
    current?: boolean;
    /** A new profile's first colour. */
    color?: number;
    /** Asked for from the sidebar: creating also switches the window to it. */
    andSwitch?: boolean;
  } = $props();

  let name = $state("");
  let choice = $state<AvatarChoice>({ style: "initials", color: 1, symbol: 0, photo: null });
  let step = $state<"edit" | "delete">("edit");
  let busy = $state(false);
  let failed = $state(false);
  let pendingDeletion = $state<string | undefined>();
  let opener: HTMLElement | null = null;
  const saved = $derived(profile ? drawn(profile).src : null);
  const valid = $derived(name.trim().length > 0);

  // Each opening starts from the profile as it is; a projection arriving
  // while the sheet is open never overwrites what is being typed.
  $effect(() => {
    if (!open) return;
    untrack(() => {
      opener ??= document.activeElement instanceof HTMLElement ? document.activeElement : null;
      step = "edit";
      failed = false;
      pendingDeletion = undefined;
      name = profile?.name ?? "";
      choice = profile
        ? {
            style: profile.avatar.style,
            color: profile.avatar.color,
            symbol: profile.avatar.symbol,
            photo: null,
          }
        : { style: "initials", color, symbol: 0, photo: null };
    });
  });

  const draft = (): AvatarDraft => ({
    style: choice.style,
    color: choice.color,
    symbol: choice.symbol,
    photo: choice.style === "photo" ? choice.photo : null,
  });

  /** A kept photo has no token; it is saved already and needs no write. */
  const avatarChanged = () =>
    !profile ||
    choice.photo !== null ||
    (choice.style !== "photo" &&
      (choice.style !== profile.avatar.style ||
        choice.color !== profile.avatar.color ||
        (choice.style === "symbol" && choice.symbol !== profile.avatar.symbol)));

  async function submit() {
    if (busy || !valid) return;
    busy = true;
    failed = false;
    let ok: boolean;
    if (!profile) ok = await profiles.create(name.trim(), draft(), andSwitch);
    else {
      ok = name.trim() === profile.name || (await profiles.rename(profile.id, name.trim()));
      if (ok && avatarChanged()) ok = await profiles.setAvatar(profile.id, draft());
    }
    busy = false;
    if (ok) open = false;
    else failed = true;
  }

  async function remove() {
    if (!profile || busy) return;
    busy = true;
    failed = false;
    const result = await profiles.remove(profile.id, pendingDeletion);
    busy = false;
    if (result.state === "completed") open = false;
    else if (result.state === "pending") pendingDeletion = result.operation_id;
    else failed = true;
  }

  const deletable = $derived(profile?.kind === "named" && !current);
</script>

<Dialog.Root bind:open>
  <Dialog.Portal>
    <Dialog.Overlay class="settings-dialog-overlay" />
    <Dialog.Content
      class="settings-dialog profile-sheet"
      onCloseAutoFocus={(event) => {
        if (opener?.isConnected) {
          event.preventDefault();
          opener.focus();
        }
        opener = null;
      }}
    >
      {#if step === "delete" && profile}
        <Dialog.Title class="settings-dialog-title"
          >{m.profiles_delete_title({ name: profile.name })}</Dialog.Title
        >
        <Dialog.Description class="settings-dialog-description"
          >{m.profiles_delete_body()}</Dialog.Description
        >
        {#if pendingDeletion}<p role="status">{m.profiles_delete_pending()}</p>{/if}
        {#if failed}<p class="failed" role="alert">{m.profiles_failed()}</p>{/if}
        <footer>
          <Button
            onclick={() => {
              if (pendingDeletion) open = false;
              else step = "edit";
            }}
            disabled={busy}>{pendingDeletion ? m.window_close() : m.action_cancel()}</Button
          >
          <Button variant="danger" pending={busy} onclick={() => void remove()}
            >{pendingDeletion ? m.profiles_delete_check() : m.profiles_delete_confirm()}</Button
          >
        </footer>
      {:else}
        <Dialog.Title class="settings-dialog-title"
          >{profile ? m.profiles_edit_title() : m.profiles_create_title()}</Dialog.Title
        >
        {#if !profile}<Dialog.Description class="settings-dialog-description"
            >{m.profiles_create_body()}</Dialog.Description
          >{/if}
        <form
          class="sheet-body"
          onsubmit={(event) => {
            event.preventDefault();
            void submit();
          }}
        >
          <Field
            label={m.profiles_name()}
            placeholder={m.profiles_name_placeholder()}
            maxlength={80}
            autocomplete="off"
            spellcheck="false"
            bind:value={name}
          />
          <AvatarPicker bind:value={choice} name={name || (profile?.name ?? "")} {saved} />
          {#if failed}<p class="failed" role="alert">{m.profiles_failed()}</p>{/if}
          <footer>
            {#if profile?.kind === "named"}<div class="start">
                <Button
                  variant="ghost"
                  disabled={!deletable || busy}
                  title={deletable ? undefined : m.profiles_delete_current()}
                  onclick={() => {
                    failed = false;
                    step = "delete";
                  }}>{m.profiles_delete_open()}</Button
                >
              </div>{/if}
            <Button onclick={() => (open = false)} disabled={busy}>{m.action_cancel()}</Button>
            <Button type="submit" variant="primary" pending={busy} disabled={!valid}
              >{profile
                ? m.profiles_save()
                : andSwitch
                  ? m.profiles_create_switch_action()
                  : m.profiles_create_action()}</Button
            >
          </footer>
        </form>
      {/if}
    </Dialog.Content>
  </Dialog.Portal>
</Dialog.Root>

<style>
  :global(.settings-dialog.profile-sheet) {
    width: min(460px, calc(100vw - 48px));
  }

  .sheet-body {
    display: grid;
    gap: 22px;
    margin-block-start: 20px;
  }

  .sheet-body footer {
    margin-top: 6px;
  }

  .start {
    margin-inline: -10px auto;
  }

  .failed {
    margin: 0;
    color: var(--color-danger);
    font-size: var(--text-label);
    line-height: 1.5;
  }
</style>
