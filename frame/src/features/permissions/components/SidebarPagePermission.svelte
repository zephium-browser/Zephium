<script lang="ts">
  import { onMount } from "svelte";
  import { pagePermissions as permissions } from "$domain/permissions";
  import * as attention from "$session/attention.svelte";
  import PagePermissionPrompt from "./PagePermissionPrompt.svelte";

  let prompt = $derived(permissions.prompt());
  $effect(() => {
    if (prompt?.in_sidebar) return attention.hold();
  });

  onMount(() => permissions.hostInSidebar());
</script>

{#if prompt?.in_sidebar}
  {#key `${prompt.profile_id}:${prompt.item_id}:${prompt.request_id}`}
    <PagePermissionPrompt {prompt} />
  {/key}
{/if}
