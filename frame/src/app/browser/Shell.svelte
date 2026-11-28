ail && !inWork ? openSwitcher : undefined}
              spaces={!inWork}
              roomy={besideRail || inWork}
            />
          {:else}
            <!--
            The switch sits above the address field because it governs the
            whole column, field included; the tray rides the field itself,
            because everything in it acts on the page the field names.
          -->
            <AddressField {compact}>
              {#snippet trailing(page)}
                <UtilityTray
                  label={page === null
                    ? m.utilities()
                    : page.warning
                      ? m.site_info_label_status({ status: page.warning })
                      : m.site_info_label()}
                  warning={page?.warning}
                >
                  {#snippet children(open)}
                    {#snippet siteHead(name: string | null, control?: Snippet)}
                      <SiteHead {page} label={name}>{@render control?.()}</SiteHead>
                    {/snippet}
                    <BlockerShield {open} head={page ? siteHead : undefined} />
                    <ExtensionsSection />
                    <SiteDetails {page} {open} />
                  {/snippet}
                </UtilityTray>
              {/snippet}
            </AddressField>
            <!-- Work floats its page over the whole window, sidebar included. -->
            {#if !inWork}<SidebarPagePermission />{/if}

            <HidingBar />
            {#if splitting}<div class="flex shrink-0 items-center gap-1 px-3 pb-1">
                <p class="flex-1 text-[12px] text-accent" aria-live="polite">{m.choose_split()}</p>
                <Button variant="ghost" bind:ref={splitCancel} onclick={() => finishSplit()}
                  >{m.action_cancel()}</Button
                >
              </div>{/if}
            <SidebarBody
              bind:this={sidebarBody}
              pinned={tree.pinned}
              today={tree.today}
              {splitting}
              onSelect={selectTab}
            >
              {#snippet extras()}<PinnedExtensions />{/snippet}
            </SidebarBody>
            <!-- Brief notices and downloads sit at the foot of the column, by the dock. -->
            <SidebarNotice />
            {#if tabs.profile()?.id}<DownloadStatus
                profile={tabs.profile()!.id}
                onopen={() => toolHost.open("downloads")}
              />{/if}
            <UpdateNotice view="cards" />
            <PrivateReturn />
            {@render player(false)}
          {/if}
        </div>{/key}
    {/snippet}{#snippet dock(
      compact,
    )}{#snippet accountFoot()}{#await loadAccountButton() then { default: AccountButton }}<AccountButton
            name={tabs.profile()?.name ?? ""}
          />{/await}{/snippet}{#if compact && incognito}{@render player(true)}<PrivateBar
          compact
        />{:else if compact}{@render player(true)}{#if tabs.profile()?.id && !inWork}<DownloadPulse
            profile={tabs.profile()?.id ?? ""}
          />{/if}<PrivateReturn compact /><UpdateNotice view="glyph" onabout={openAbout} /><Dock
          compact
          roomy={besideRail}
          tools={!inWork}
          foot={inWork ? accountFoot : undefined}
        >
          {#snippet sites()}<EssentialsRail
              entries={railEssentials}
              onSelect={selectTab}
            />{/snippet}
        </Dock>{:else if incognito}
        <!-- A private window keeps nothing, so it has no tools or kept sites;
             its foot names the scope and closes it. -->
        <PrivateBar />{:else}<Dock>
          {#snippet above()}
            <div
              class="dock-sites"
              data-essentials-drop
              data-over={tabDrag.overEssentials()}
              hidden={keptAbove.length === 0}
            >
              <TabList
                entries={keptAbove}
                section="favorites"
                variant="essentials"
                columns={dockColumns}
                label={m.essentials()}
                {splitting}
                onSelect={selectEssential}
                >{#snippet essentialTile(props)}<EssentialTile {...props} />{/snippet}</TabList
              >
            </div>
          {/snippet}
          {#snippet sites()}
            <div
              class="dock-sites"
              bind:clientWidth={sitesWidth}
              data-essentials-drop
              data-over={tabDrag.overEssentials()}
              data-empty={tree.favorites.length === 0}
            >
              {#if tree.favorites.length === 0}<span class="dock-sites-hint"
                  >{m.essential_drop_hint()}</span
                >{/if}
              <TabList
                entries={keptBeside}
                section="favorites"
                variant="essentials"
                label={m.essentials()}
                {splitting}
                onSelect={selectEssential}
                >{#snippet essentialTile(props)}<EssentialTile {...props} />{/snippet}</TabList
              >
            </div>
            {#if tabDrag.moveFailed()}<p class="sidebar-move-error" role="alert">
                {m.essential_move_failed()}
              </p>{/if}
          {/snippet}
        </Dock>{/if}{/snippet}{#snippet settingsNavigation()}<SettingsNavigation
      />{/snippet}{#snippet toolPanel(kind)}<LazyView
        loader={loadToolSlot}
        loadingLabel={m.surface_loading()}
        failureLabel={m.surface_render_failed()}
        retryLabel={m.surface_retry()}
        >{#snippet children(View)}<View
            tool={kind}
            profile={tabs.profile()?.id ?? "unbound"}
            onclose={toolHost.close}
          />{/snippet}</LazyView
      >{/snippet}</Sidebar
  >
  <div class="stage-column">
    {#if browserPage.navigationFailed()}<div class="navigation-error" role="alert">
        {m.browser_nav_failed()}
      </div>{/if}
    {#if focus.cover() !== null && browserPage.currentPage() === null}
      <!-- Focus shuts the site this tab shows; native has taken the page off
         the stage, so this stands where it was. -->
      <main class="internal-stage">
        <div class="stage-page">
          <LazyView
            loader={loadFocusCover}
            loadingLabel={m.surface_loading()}
            failureLabel={m.surface_render_failed()}
            retryLabel={m.surface_retry()}
            >{#snippet children(View)}<View site={focus.cover() ?? ""} />{/snippet}</LazyView
          >
        </div>
      </main>
    {:else if (tabs.activeTab()?.availability?.state === "waiting_for_capacity" || tabs.activeTab()?.availability?.state === "blocked_by_capacity") && browserPage.currentPage() === null}
      {@const capacityTab = tabs.activeTab()}
      <main class="internal-stage">
        {#if capacityTab}<LazyView
            loader={loadTabCapacityState}
            loadingLabel={m.surface_loading()}
            failureLabel={m.surface_render_failed()}
            retryLabel={m.surface_retry()}
            >{#snippet children(View)}<View tab={capacityTab} />{/snippet}</LazyView
          >{/if}
      </main>
    {:else if tabs.activeTab() && failedPane(tabs.activeTab()!) && browserPage.currentPage() === null}
      {@const failedTab = tabs.activeTab()}
      {@const failure = failedTab?.failure}
      <!-- A page that could not be shown: the pane says why instead of staying blank. -->
      <main class="min-w-0 flex-1 ps-2">
        <div class="content-pane h-full w-full overflow-hidden">
          {#if failedTab && failure}<LazyView
              loader={loadNavigationError}
              loadingLabel={m.surface_loading()}
              failureLabel={m.surface_render_failed()}
              retryLabel={m.surface_retry()}
              >{#snippet children(View)}<View tab={failedTab} {failure} />{/snippet}</LazyView
            >{/if}
        </div>
      </main>
    {:else if newTabShown}
      <!--
      A tab opened straight to an address is loading before it has a URL; it
      goes to its page rather than flashing the new tab first.
      Occupies exactly the rect a content WebView would, so moving between a
      page and the new tab never changes the window's shape. The inline start
      inset matches the core layout gap between chrome and content.
    -->
      <main class="min-w-0 flex-1 ps-2" data-zephium-new-tab>
        <!-- New Tab paints its own ground, so its dock can open onto the frame. -->
        <div class="content-pane h-full w-full overflow-hidden" data-ground="own">
          <LazyView
            loader={loadNewTab}
            loadingLabel={m.surface_loading()}
            failureLabel={m.surface_render_failed()}
            retryLabel={m.surface_retry()}
            >{#snippet children(NewTab)}<NewTab
                oncustomize={openSettings}
                onaccount={openAccount}
                ontasks={() => void browserPage.open("tasks")}
                ontime={() => void browserPage.open("time")}
                >{#snippet search()}{#key tabs.activeId()}<LazyView
                      loader={loadNewTabSearch}
                      loadingLabel={m.surface_loading()}
                      failureLabel={m.surface_render_failed()}
                      retryLabel={m.surface_retry()}
                      >{#snippet children(Search)}<Search
                          tabId={tabs.activeId()}
                          {loadMenu}
                          onsettings={openSearchSettings}
                        />{/snippet}</LazyView
                    >{/key}{/snippet}</NewTab
              >{/snippet}</LazyView
          >
        </div>
      </main>
    {/if}
    {#if browserPage.currentPage() !== null}
      <main class="internal-stage">
        <!-- Each destination arrives as its own page: the ground settles in
           while its content rises onto it. -->
        {#key browserPage.currentPage()}<div
            class="stage-page"
            data-page={browserPage.currentPage()}
          >
            <RenderBoundary title={m.surface_render_failed()} retryLabel={m.surface_retry()}>
              {#if browserPage.currentPage() === "settings"}
                <LazyView
                  loader={loadSettings}
                  loadingLabel={m.surface_loading()}
                  failureLabel={m.surface_render_failed()}
                  retryLabel={m.surface_retry()}
                  >{#snippet children(View)}<View />{/snippet}</LazyView
                >
              {:else if browserPage.currentPage() === "extensions"}
                <LazyView
                  loader={loadWebExtensionManager}
                  loadingLabel={m.surface_loading()}
                  failureLabel={m.surface_render_failed()}
                  retryLabel={m.surface_retry()}
                  >{#snippet children(View)}<View />{/snippet}</LazyView
                >
              {:else if browserPage.currentPage() === "work"}
                {#key tabs.profile()?.id}<LazyView
                    loader={loadWorkWorkspace}
                    loadingLabel={m.surface_loading()}
                    failureLabel={m.surface_render_failed()}
                    retryLabel={m.surface_retry()}
                    >{#snippet children(View)}<View />{/snippet}</LazyView
                  >{/key}
              {:else if browserPage.currentPage() === "notes"}
                {#key tabs.profile()?.id}<LazyView
                    loader={loadNotesPage}
                    loadingLabel={m.surface_loading()}
                    failureLabel={m.surface_render_failed()}
                    retryLabel={m.surface_retry()}
                    >{#snippet children(View)}<View
                        profile={tabs.profile()?.id ?? ""}
                        onclose={() => void browserPage.close()}
                      />{/snippet}</LazyView
                  >{/key}
              {:else if browserPage.currentPage() === "tasks"}
                <LazyView
                  loader={loadTasksPage}
                  loadingLabel={m.surface_loading()}
                  failureLabel={m.surface_render_failed()}
                  retryLabel={m.surface_retry()}
                  >{#snippet children(View)}<View />{/snippet}</LazyView
                >
              {:else if browserPage.currentPage() === "time"}
                {#key tabs.profile()?.id}<LazyView
                    loader={loadTimePage}
                    loadingLabel={m.surface_loading()}
                    failureLabel={m.surface_render_failed()}
                    retryLabel={m.surface_retry()}
                    >{#snippet children(View)}<View />{/snippet}</LazyView
                  >{/key}
              {:else if browserPage.currentPage() === "history"}
                <LazyView
                  loader={loadHistoryPage}
                  loadingLabel={m.surface_loading()}
                  failureLabel={m.surface_render_failed()}
                  retryLabel={m.surface_retry()}
                  >{#snippet children(View)}<View />{/snippet}</LazyView
                >
              {:else if browserPage.currentPage() === "source"}
                {#key tabs.activeId()}<LazyView
                    loader={loadSourcePage}
                    loadingLabel={m.surface_loading()}
                    failureLabel={m.surface_render_failed()}
                    retryLabel={m.surface_retry()}
                    >{#snippet children(View)}<View
                        tab={tabs.activeId() ?? ""}
                      />{/snippet}</LazyView
                  >{/key}
              {:else}
                <LazyView
                  loader={loadLibraryPage}
                  loadingLabel={m.surface_loading()}
                  failureLabel={m.surface_render_failed()}
                  retryLabel={m.surface_retry()}
                  >{#snippet children(View)}<View kind="downloads" />{/snippet}</LazyView
                >{/if}
            </RenderBoundary>
          </div>{/key}
      </main>
    {/if}
  </div>
</div>
{#if ClearSiteData}<ClearSiteData />{/if}
{#if iconPicker && EssentialIconPicker}<EssentialIconPicker
    id={iconPicker.id}
    title={iconPicker.title}
    current={iconPicker.current}
    onclose={() => (iconPicker = null)}
  />{/if}

<style>
  .stage-column {
    display: flex;
    flex: 1;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
  }

  .stage-column > :global(main) {
    flex: 1;
    min-height: 0;
  }

  .rail-search {
    display: flex;
    justify-content: center;
    margin-block: 8px 14px;
  }

  .sidebar-mode-body {
    display: flex;
    flex: 1;
    flex-direction: column;
    min-height: 0;
  }

  .sidebar-mode-body[data-arriving="true"] {
    /* Release the opacity backdrop root after entry so descendant menus can blur. */
    animation: mode-body-in var(--motion-slow) var(--ease-emphasized) backwards;
  }

  @keyframes mode-body-in {
    from {
      opacity: 0;
      translate: 0 6px;
    }
  }
</style>
