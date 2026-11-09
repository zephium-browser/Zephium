//! Source anchors for privileged frame markup and bootstrap contract tests.
//! A frontend move changes its path here; assertions stay with the native contract.

pub const SRC_FEATURES_DOCK_TOOLSHELF_SVELTE: &str =
    include_str!("../../frame/src/features/dock/components/ToolShelf.svelte");
pub const SRC_STYLES_TOKENS_CSS: &str = include_str!("../../frame/src/styles/tokens.css");
pub const SRC_BOOTSTRAP_CSS: &str = include_str!("../../frame/src/styles/axes/bootstrap.css");
pub const PANEL_HTML: &str = include_str!("../../frame/panel.html");
pub const LITTLE_HTML: &str = include_str!("../../frame/little.html");
pub const INDEX_HTML: &str = include_str!("../../frame/browser.html");
pub const ONBOARDING_HTML: &str = include_str!("../../frame/onboarding.html");
pub const SRC_APP_APP_SVELTE: &str = include_str!("../../frame/src/app/browser/BrowserApp.svelte");
pub const SRC_FEATURES_SIDEBAR_ADDRESS_ADDRESSFIELD_SVELTE: &str =
    include_str!("../../frame/src/features/address/components/AddressField.svelte");
pub const SRC_FEATURES_SIDEBAR_TABS_TABRAIL_SVELTE: &str =
    include_str!("../../frame/src/features/tabs/components/TabRail.svelte");
pub const SRC_FEATURES_SIDEBAR_ESSENTIALS_ESSENTIALSRAIL_SVELTE: &str =
    include_str!("../../frame/src/features/essentials/components/EssentialsRail.svelte");
pub const SRC_FEATURES_SIDEBAR_TABS_TABLIST_SVELTE: &str =
    include_str!("../../frame/src/features/tabs/components/TabList.svelte");
pub const SRC_DOMAIN_TABS_TABS_SVELTE_TS: &str =
    include_str!("../../frame/src/domain/tabs/tabs.svelte.ts");
pub const SRC_MAIN_TS: &str = include_str!("../../frame/src/entries/browser.ts");
pub const SRC_APP_SHELL_SVELTE: &str = include_str!("../../frame/src/app/browser/Shell.svelte");
pub const SRC_FEATURES_SIDEBAR_TABS_TABROW_SVELTE: &str =
    include_str!("../../frame/src/features/tabs/components/TabRow.svelte");
pub const SRC_FEATURES_SIDEBAR_TABS_SPLITGROUPROW_SVELTE: &str =
    include_str!("../../frame/src/features/tabs/components/SplitGroupRow.svelte");
pub const SRC_SHARED_LIB_LOCALE_SVELTE_TS: &str =
    include_str!("../../frame/src/shared/lib/locale.svelte.ts");
