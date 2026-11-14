//! Chrome extension runtime on system WebKitGTK.
//!
//! The browser owns installation, consent and its tab model; the shared
//! `zephium-webext-runtime` owns Chrome semantics. This crate owns everything
//! between those and WebKitGTK: the extension origin, extension-owned views,
//! content-script injection and the bridge to the web-process module.
#![cfg(target_os = "linux")]

pub mod background;
pub mod bridge;
pub mod csp;
pub mod network;
pub mod offscreen;
pub mod package;
pub mod runtime;
pub mod scheme;
pub mod views;
