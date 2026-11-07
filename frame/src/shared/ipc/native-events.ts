import type {
  WorkEnvironmentChanged,
  WorkChanged,
  WorkHumanChanged,
  WorkDecisionPreferenceChanged,
  WorkModelsChanged,
  AccountChanged,
  PanelState,
  PageAssistChanged,
  ProfilesView,
  FaviconsView,
  NoteOpenRequested,
  NoteChanges,
  ResourceChanged,
  DownloadsChanged,
  BlockerStatusChanged,
  BrowserCredentialCapabilityChanged,
  FocusChanged,
  ExtensionActionFailed,
  ExtensionActionShortcut,
  ExtensionActionsChanged,
  FindResultView,
  ImportJobView,
  ItemsChanged,
  KeymapChanged,
  LayoutChanged,
  MediaPlayerChanged,
  OperationProcessed,
  PagePermissionPromptChanged,
  RuntimeStatusChanged,
  SearchChanged,
  TabChanged,
  UiCommand,
  VoiceChanged,
  VoiceLevels,
  SwipeScroll,
  WebExtensionAccessRequested,
  WebExtensionStoreClicked,
} from "./bindings";

type PayloadEvent<T> = { payload: T };
type Listener<T> = (event: PayloadEvent<T>) => void;
type PresentationTab = { tab: TabChanged; active: string | null };

function scopedEvent<T>(name: string) {
  return {
    listen(listener: Listener<T>): Promise<() => void> {
      const handler = (event: Event) => listener({ payload: (event as CustomEvent<T>).detail });
      window.addEventListener(name, handler);
      return Promise.resolve(() => window.removeEventListener(name, handler));
    },
  };
}

// Rust delivers these directly into the intended WebView. Do not replace this
// with @tauri-apps/api/event: its listen command accepts a caller-selected
// target, which would let the launcher panel subscribe to main-window state.
export const nativeEventNames = {
  windowsReputationManaged: "zephium:windows-reputation-managed",
  workEnvironmentChanged: "zephium:work-environment-changed",
  workChanged: "zephium:work-changed",
  workHumanChanged: "zephium:work-human-changed",
  workDecisionPreferenceChanged: "zephium:work-decision-preference-changed",
  workModelsChanged: "zephium:work-models-changed",
  pageAssistChanged: "zephium:page-assist-changed",
  accountChanged: "zephium:account-changed",
  panelState: "zephium:panel-state",
  favicons: "zephium:favicons",
  noteOpenRequested: "zephium:note-open-requested",
  resourceChanged: "zephium:resource-changed",
  notesChanged: "zephium:notes-changed",
  downloadsChanged: "zephium:downloads-changed",
  resourceClose: "zephium:resource-close",
  resourceCloseCancelled: "zephium:resource-close-cancelled",
  browserCredentialCapabilityChanged: "zephium:browser-credential-capability",
  itemsChanged: "zephium:items",
  tabChanged: "zephium:tab",
  extensionActionsChanged: "zephium:extension-actions",
  extensionActionFailed: "zephium:extension-action-failed",
  extensionActionShortcut: "zephium:extension-action-shortcut",
  pagePermissionPromptChanged: "zephium:page-permission-prompt",
  webExtensionAccessRequested: "zephium:web-extension-access",
  webExtensionStoreClicked: "zephium:web-extension-store-click",
  webExtensionDropped: "zephium:web-extension-dropped",
  browserReturn: "zephium:browser-return",
  presentationTab: "zephium:presentation-tab",
  uiCommand: "zephium:ui-command",
  keymapChanged: "zephium:keymap-changed",
  importProgress: "zephium:import-progress",
  find: "zephium:find",
  searchChanged: "zephium:search",
  layoutChanged: "zephium:layout",
  mediaPlayerChanged: "zephium:media-player",
  operationProcessed: "zephium:operation-processed",
  runtimeStatusChanged: "zephium:runtime-status",
  blockerStatusChanged: "zephium:blocker-status",
  focusChanged: "zephium:focus",
  voiceChanged: "zephium:voice-changed",
  voiceLevels: "zephium:voice-levels",
  swipeScroll: "zephium:swipe",
  profilesChanged: "zephium:profiles",
} as const;

export const events = {
  windowsReputationManaged: scopedEvent<boolean>(nativeEventNames.windowsReputationManaged),
  workEnvironmentChanged: scopedEvent<WorkEnvironmentChanged>(
    nativeEventNames.workEnvironmentChanged,
  ),
  workChanged: scopedEvent<WorkChanged>(nativeEventNames.workChanged),
  workHumanChanged: scopedEvent<WorkHumanChanged>(nativeEventNames.workHumanChanged),
  workDecisionPreferenceChanged: scopedEvent<WorkDecisionPreferenceChanged>(
    nativeEventNames.workDecisionPreferenceChanged,
  ),
  workModelsChanged: scopedEvent<WorkModelsChanged>(nativeEventNames.workModelsChanged),
  pageAssistChanged: scopedEvent<PageAssistChanged>(nativeEventNames.pageAssistChanged),
  accountChanged: scopedEvent<AccountChanged>(nativeEventNames.accountChanged),
  favicons: scopedEvent<FaviconsView>(nativeEventNames.favicons),
  noteOpenRequested: scopedEvent<NoteOpenRequested>(nativeEventNames.noteOpenRequested),
  resourceClose: scopedEvent<string>(nativeEventNames.resourceClose),
  resourceCloseCancelled: scopedEvent<string>(nativeEventNames.resourceCloseCancelled),
  resourceChanged: scopedEvent<ResourceChanged>(nativeEventNames.resourceChanged),
  notesChanged: scopedEvent<NoteChanges>(nativeEventNames.notesChanged),
  downloadsChanged: scopedEvent<DownloadsChanged>(nativeEventNames.downloadsChanged),
  panelState: scopedEvent<PanelState>(nativeEventNames.panelState),
  browserCredentialCapabilityChanged: scopedEvent<BrowserCredentialCapabilityChanged>(
    nativeEventNames.browserCredentialCapabilityChanged,
  ),
  itemsChanged: scopedEvent<ItemsChanged>(nativeEventNames.itemsChanged),
  tabChanged: scopedEvent<TabChanged>(nativeEventNames.tabChanged),
  extensionActionsChanged: scopedEvent<ExtensionActionsChanged>(
    nativeEventNames.extensionActionsChanged,
  ),
  extensionActionFailed: scopedEvent<ExtensionActionFailed>(nativeEventNames.extensionActionFailed),
  extensionActionShortcut: scopedEvent<ExtensionActionShortcut>(
    nativeEventNames.extensionActionShortcut,
  ),
  pagePermissionPromptChanged: scopedEvent<PagePermissionPromptChanged>(
    nativeEventNames.pagePermissionPromptChanged,
  ),
  webExtensionAccessRequested: scopedEvent<WebExtensionAccessRequested>(
    nativeEventNames.webExtensionAccessRequested,
  ),
  webExtensionStoreClicked: scopedEvent<WebExtensionStoreClicked>(
    nativeEventNames.webExtensionStoreClicked,
  ),
  /** The path of an extension package dropped on the browser. */
  webExtensionDropped: scopedEvent<string>(nativeEventNames.webExtensionDropped),
  browserReturn: scopedEvent<ItemsChanged>(nativeEventNames.browserReturn),
  presentationTab: scopedEvent<PresentationTab>(nativeEventNames.presentationTab),
  uiCommand: scopedEvent<UiCommand>(nativeEventNames.uiCommand),
  keymapChanged: scopedEvent<KeymapChanged>(nativeEventNames.keymapChanged),
  importProgress: scopedEvent<ImportJobView>(nativeEventNames.importProgress),
  find: scopedEvent<FindResultView>(nativeEventNames.find),
  searchChanged: scopedEvent<SearchChanged>(nativeEventNames.searchChanged),
  layoutChanged: scopedEvent<LayoutChanged>(nativeEventNames.layoutChanged),
  mediaPlayerChanged: scopedEvent<MediaPlayerChanged>(nativeEventNames.mediaPlayerChanged),
  operationProcessed: scopedEvent<OperationProcessed>(nativeEventNames.operationProcessed),
  runtimeStatusChanged: scopedEvent<RuntimeStatusChanged>(nativeEventNames.runtimeStatusChanged),
  blockerStatusChanged: scopedEvent<BlockerStatusChanged>(nativeEventNames.blockerStatusChanged),
  focusChanged: scopedEvent<FocusChanged>(nativeEventNames.focusChanged),
  voiceChanged: scopedEvent<VoiceChanged>(nativeEventNames.voiceChanged),
  voiceLevels: scopedEvent<VoiceLevels>(nativeEventNames.voiceLevels),
  swipeScroll: scopedEvent<SwipeScroll>(nativeEventNames.swipeScroll),
  profilesChanged: scopedEvent<ProfilesView>(nativeEventNames.profilesChanged),
};
