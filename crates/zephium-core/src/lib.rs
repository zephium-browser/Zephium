#![forbid(unsafe_code)]

pub mod accelerator;
/// Whether a tab is making sound, and whether the person muted it.
pub mod audio;
pub mod blocker;
pub mod bookmarks;
pub mod browsing_data;
pub mod certificates;
pub mod commands;
pub mod downloads;
pub mod export;
pub mod extension_data;
pub mod extensions;
pub mod geometry;
pub mod http_auth;
pub mod icon;
pub mod ids;
pub mod injection;
pub mod item;
pub mod items;
pub mod layout;
pub mod macos;
/// What a tab's media is doing, and which tab the sidebar's player follows.
pub mod media;
pub mod navigation;
/// Markdown notes the person owns, described by an index built from the files.
pub mod notes;
pub mod permissions;
pub mod ports;
pub mod profiles;
pub mod proxy;
pub mod runtime_security;
pub mod search;
pub mod session;
pub mod spaces;
pub mod split;
/// Time on the web and focus sessions.
pub mod time;
pub mod userscripts;
pub mod webkitgtk;
pub mod webview2;
pub mod windows;
pub mod work;
/// Page zoom levels and their bounds.
pub mod zoom;

/// Allowlisted application appearance preferences.
pub mod preferences;
/// Colour themes and the window ground native derives from them.
pub mod theme;

/// Durable user-authored resources shared by Browse and Work.
pub mod resources;
