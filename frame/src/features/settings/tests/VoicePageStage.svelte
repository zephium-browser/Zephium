<script lang="ts">
  import VoicePage from "../components/sections/VoicePage.svelte";
</script>

<!-- The Settings shell's own page frame around the section, as Settings.svelte draws it. -->
<section class="settings-shell" style="height: 100vh">
  <div class="settings-content scrolls">
    <div class="settings-page">
      <div class="settings-view">
        <header class="settings-page-heading">
          <h1>Voice</h1>
          <p>Talk to Zephium and hear it answer.</p>
        </header>
        <VoicePage />
      </div>
    </div>
  </div>
</section>
