//! Trusted desktop Work composition, without Tauri, UI or probe authority.
#![forbid(unsafe_code)]
#![deny(clippy::dbg_macro, clippy::print_stderr, clippy::print_stdout)]

#[cfg(all(
    feature = "native-work",
    not(any(target_os = "macos", target_os = "windows", target_os = "linux"))
))]
compile_error!("native Work composition requires macOS, Windows or Linux");
#[cfg(all(feature = "public-qualification", not(debug_assertions)))]
compile_error!("public Work qualification is forbidden in optimized builds");
#[cfg(all(
    feature = "retained-action-qualification",
    any(
        feature = "retained-commerce-qualification",
        feature = "discovery-qualification",
        feature = "retained-notion-qualification",
        feature = "retained-notion-write-qualification"
    )
))]
compile_error!("select the retained local action qualification without another objective");
#[cfg(all(
    feature = "retained-notion-qualification",
    any(
        feature = "retained-commerce-qualification",
        feature = "discovery-qualification",
        feature = "retained-notion-write-qualification"
    )
))]
compile_error!("select the authenticated Notion qualification without another objective");
#[cfg(all(
    feature = "retained-notion-write-qualification",
    any(
        feature = "retained-commerce-qualification",
        feature = "discovery-qualification"
    )
))]
compile_error!("select the authenticated Notion write qualification without another objective");

#[cfg(feature = "durable-runtime")]
mod account_scope;
#[cfg(feature = "durable-runtime")]
pub mod durable_runtime;
#[cfg(feature = "native-work")]
mod native;
#[cfg(feature = "native-work")]
mod native_work_clock;
#[cfg(feature = "native-work")]
mod open_objective;
#[cfg(feature = "durable-runtime")]
pub mod page_assist;
#[cfg(feature = "macos-work")]
pub use native::NativeWorkComposition as MacosWorkComposition;
#[cfg(feature = "native-work")]
pub use native::{NativeWorkComposition, TrustedWorkRequest};
#[cfg(feature = "native-work")]
pub use open_objective::{
    PublicLocalActionWorkInvocation, PublicReadWorkAccount, PublicReadWorkInvocation,
    PublicReadWorkObjective, PublicReadWorkSettings,
};
#[cfg(feature = "native-work")]
pub use zephium_agent_controller::AgentWorkFailure;
#[cfg(feature = "public-qualification")]
mod qualification;
#[cfg(feature = "retained-action-qualification")]
#[doc(hidden)]
pub mod retained_action_qualification;
#[cfg(feature = "retained-back-qualification")]
#[doc(hidden)]
pub mod retained_back_qualification;
#[cfg(feature = "retained-notion-qualification")]
#[doc(hidden)]
pub mod retained_notion_qualification;
#[cfg(any(
    feature = "retained-notion-qualification",
    feature = "retained-notion-write-qualification"
))]
mod retained_notion_support;
#[cfg(feature = "retained-notion-write-qualification")]
#[doc(hidden)]
pub mod retained_notion_write_qualification;
#[cfg(feature = "retained-product-qualification")]
#[doc(hidden)]
pub mod retained_product_qualification;
#[cfg(all(feature = "navigation-qualification", not(debug_assertions)))]
compile_error!("navigation Work qualification is forbidden in optimized builds");
#[cfg(feature = "discovery-qualification")]
#[doc(hidden)]
pub mod discovery_qualification;
#[cfg(feature = "navigation-qualification")]
#[doc(hidden)]
pub mod navigation_qualification;
#[cfg(feature = "retained-qualification")]
#[doc(hidden)]
pub mod retained_qualification;

#[cfg(feature = "durable-runtime")]
mod human;
#[cfg(feature = "durable-runtime")]
pub use human::WaitingHumanPage;
