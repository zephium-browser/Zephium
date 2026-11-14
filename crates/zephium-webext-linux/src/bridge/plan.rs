//! What one view may host, as the host decided it, and its wire form.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use sha2::{Digest, Sha256};
use url::Url;
use zephium_core::ids::ExtensionInstallId;
use zephium_webext_runtime::context::{ExtensionId, ExtensionScheme, InjectedWorld};
use zephium_webext_runtime::limits::{
    MAX_IN_FLIGHT_BYTES_PER_CONTEXT, MAX_MESSAGE_BYTES_IN_WEB_RENDERER,
    MAX_MESSAGE_BYTES_RUNTIME_OWNED, MAX_OUTSTANDING_CALLS_PER_CONTEXT, RATE_BURST,
    RATE_MESSAGES_PER_SECOND,
};
use zephium_webkit_web_process_protocol::protocol::{
    encode_ui, Attach, ExternalRule, Limits, MatchRule, PageRole, UiMessage, WorldRule,
    MAX_ATTACH_BYTES, MAX_BOOTSTRAP_BYTES, MAX_ORIGINS, MAX_WORLDS, PROTOCOL_VERSION,
};

const ISOLATED_WORLD_PREFIX: &str = "zephium-principal-extension-";
const USER_SCRIPT_WORLD_PREFIX: &str = "zephium-webext-us-";

// The module caps a page at 128 MiB, 2,048 outstanding posts, 16,000 posts
// per second and a burst of 4,000; frames get the runtime's per-context values.
const PAGE_IN_FLIGHT_BYTES: u64 = 128 * 1024 * 1024;
const PAGE_OUTSTANDING: u16 = 2048;
const PAGE_MESSAGES_PER_SECOND: u32 = 16_000;
const PAGE_BURST: u16 = 4000;

const _: () = assert!(MAX_MESSAGE_BYTES_RUNTIME_OWNED <= PAGE_IN_FLIGHT_BYTES);
const _: () = assert!(MAX_IN_FLIGHT_BYTES_PER_CONTEXT <= PAGE_IN_FLIGHT_BYTES);
const _: () = assert!(PAGE_IN_FLIGHT_BYTES <= u32::MAX as u64);
const _: () = assert!(MAX_OUTSTANDING_CALLS_PER_CONTEXT <= PAGE_OUTSTANDING as u32);
const _: () = assert!(RATE_MESSAGES_PER_SECOND <= PAGE_MESSAGES_PER_SECOND);
const _: () = assert!(RATE_BURST <= PAGE_BURST as u32);

/// The script world name the module installs the bridge in, and the name
/// content-script injection must use for the same world.
pub fn world_name(install: ExtensionInstallId, world: &InjectedWorld) -> String {
    let install = hex(&install.bytes());
    match world {
        InjectedWorld::Isolated => format!("{ISOLATED_WORLD_PREFIX}{install}"),
        InjectedWorld::UserScript(id) => {
            let digest = Sha256::digest(id.as_str().as_bytes());
            format!("{USER_SCRIPT_WORLD_PREFIX}{install}-{}", hex(&digest[..16]))
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    bytes
        .iter()
        .flat_map(|byte| {
            [
                DIGITS[usize::from(byte >> 4)],
                DIGITS[usize::from(byte & 15)],
            ]
        })
        .map(char::from)
        .collect()
}

/// One install's world and the compiled declarations admitting its documents.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorldGrant {
    /// The install whose scripts run in the world.
    pub install: ExtensionInstallId,
    /// Which of its worlds.
    pub world: InjectedWorld,
    /// Chrome match declarations compiled by the shared runtime matcher.
    pub matches: Vec<MatchRule>,
    /// Glue run in the world at document creation, before page script.
    pub bootstrap: Arc<str>,
}

/// An extension origin whose pages a view may load in its default world.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtensionPages {
    /// The extension.
    pub extension: ExtensionId,
    /// Manifest `sandbox.pages` paths (`/sandbox.html`). Documents at these
    /// paths have an opaque origin and get no API.
    pub sandboxed_paths: Vec<String>,
}

/// Everything one view may host. The host builds it from installs, grants
/// and site access; nothing in it comes from the page.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AttachPlan {
    /// Isolated and `userScripts` worlds.
    pub worlds: Vec<WorldGrant>,
    /// Restricted page-world external messaging declarations.
    pub external: Vec<ExternalRule>,
    /// Extension origins whose documents get the bridge in the page world.
    pub extension_pages: Vec<ExtensionPages>,
    /// Glue run in the page world of those documents.
    pub extension_bootstrap: Arc<str>,
    /// Report request observations.
    pub observe_requests: bool,
    /// Report form submissions.
    pub observe_forms: bool,
    /// Apply the profile's request-policy snapshot.
    pub apply_policy: bool,
}

/// Why a plan was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlanError {
    /// More worlds or origins than the module accepts.
    TooMany,
    /// An origin that is not an exact HTTP(S) or extension origin.
    InvalidOrigin(String),
    /// Two grants for the same world.
    DuplicateWorld,
    /// A world with no match declarations.
    EmptyWorld,
    /// A bootstrap over the module's limit or containing NUL.
    InvalidBootstrap,
    /// The encoded attachment exceeds the module's limit.
    TooLarge,
    /// The profile's extension scheme is not one the module serves.
    UnsupportedScheme,
    /// A sandboxed path that is not an absolute path.
    InvalidSandboxPath,
    /// An extension-owned view may host only its own extension's pages.
    NotOwnPages,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ResolvedWorld {
    pub install: ExtensionInstallId,
    pub world: InjectedWorld,
    pub matches: Vec<MatchRule>,
    pub bootstrap: Arc<str>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ResolvedPages {
    pub sandboxed: BTreeSet<String>,
}

/// A validated plan, keyed the way posts are checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ResolvedPlan {
    pub worlds: BTreeMap<String, ResolvedWorld>,
    pub external: Vec<ExternalRule>,
    pub pages: BTreeMap<String, ResolvedPages>,
    pub extension_bootstrap: Arc<str>,
    pub observe_requests: bool,
    pub observe_forms: bool,
    pub apply_policy: bool,
}

impl ResolvedPlan {
    pub fn resolve(
        plan: &AttachPlan,
        scheme: ExtensionScheme,
        owner: Option<&ExtensionId>,
    ) -> Result<Self, PlanError> {
        if plan.worlds.len() > MAX_WORLDS || plan.extension_pages.len() > MAX_ORIGINS {
            return Err(PlanError::TooMany);
        }
        if !plan.extension_pages.is_empty() && scheme != ExtensionScheme::CHROME_EXTENSION {
            return Err(PlanError::UnsupportedScheme);
        }
        if let Some(owner) = owner {
            let own = plan.worlds.is_empty()
                && plan.external.is_empty()
                && plan.extension_pages.len() == 1
                && &plan.extension_pages[0].extension == owner;
            if !own {
                return Err(PlanError::NotOwnPages);
            }
        }
        bootstrap(&plan.extension_bootstrap)?;
        let mut worlds = BTreeMap::new();
        for grant in &plan.worlds {
            bootstrap(&grant.bootstrap)?;
            if grant.matches.is_empty() {
                return Err(PlanError::EmptyWorld);
            }
            if grant.matches.len() > MAX_ORIGINS {
                return Err(PlanError::TooMany);
            }
            let resolved = ResolvedWorld {
                install: grant.install,
                world: grant.world.clone(),
                matches: grant.matches.clone(),
                bootstrap: grant.bootstrap.clone(),
            };
            if worlds
                .insert(world_name(grant.install, &grant.world), resolved)
                .is_some()
            {
                return Err(PlanError::DuplicateWorld);
            }
        }
        let mut pages = BTreeMap::new();
        for page in &plan.extension_pages {
            let origin = format!("{}://{}", scheme.as_str(), page.extension.as_str());
            let mut sandboxed = BTreeSet::new();
            for path in &page.sandboxed_paths {
                let valid = path.starts_with('/')
                    && Url::parse(&format!("{origin}{path}"))
                        .is_ok_and(|url| url.path() == path && url.query().is_none());
                if !valid {
                    return Err(PlanError::InvalidSandboxPath);
                }
                sandboxed.insert(path.clone());
            }
            pages.insert(origin, ResolvedPages { sandboxed });
        }
        let resolved = Self {
            worlds,
            external: plan.external.clone(),
            pages,
            extension_bootstrap: plan.extension_bootstrap.clone(),
            observe_requests: plan.observe_requests,
            observe_forms: plan.observe_forms,
            apply_policy: plan.apply_policy,
        };
        let probe = resolved.attach("probe", 1, PageRole::OrdinaryTab);
        match encode_ui(&UiMessage::Attach(probe)) {
            Ok(bytes) if bytes.len() <= MAX_ATTACH_BYTES => Ok(resolved),
            _ => Err(PlanError::TooLarge),
        }
    }

    /// Uses the same compiled match declarations as the module.
    pub fn admits_world(
        &self,
        name: &str,
        facts: &zephium_webkit_web_process_protocol::protocol::FrameInfo,
    ) -> Option<&ResolvedWorld> {
        self.worlds
            .get(name)
            .filter(|world| world.matches.iter().any(|rule| rule.matches(facts)))
    }

    pub fn admits_external(
        &self,
        install: &str,
        facts: &zephium_webkit_web_process_protocol::protocol::FrameInfo,
    ) -> bool {
        self.external
            .iter()
            .any(|rule| rule.install_id == install && rule.admits(facts))
    }

    /// A conservative structural widening keeps existing native contexts alive.
    pub fn widens(&self, old: &Self) -> bool {
        self.extension_bootstrap == old.extension_bootstrap
            && self.observe_requests == old.observe_requests
            && self.observe_forms == old.observe_forms
            && self.apply_policy == old.apply_policy
            && old.worlds.iter().all(|(name, prior)| {
                self.worlds.get(name).is_some_and(|next| {
                    next.install == prior.install
                        && next.world == prior.world
                        && next.bootstrap == prior.bootstrap
                        && prior.matches.iter().all(|rule| next.matches.contains(rule))
                })
            })
            && old
                .pages
                .iter()
                .all(|(origin, pages)| self.pages.get(origin) == Some(pages))
            && old.external.iter().all(|rule| self.external.contains(rule))
    }

    /// Whether the page world of a document of `origin` gets the bridge.
    pub fn admits_page(&self, origin: &str) -> Option<&ResolvedPages> {
        self.pages.get(origin)
    }

    pub fn attach(&self, token: &str, generation: u64, role: PageRole) -> Attach {
        Attach {
            protocol_version: PROTOCOL_VERSION,
            token: token.to_owned(),
            generation,
            role,
            limits: limits(role),
            worlds: self
                .worlds
                .iter()
                .map(|(name, world)| WorldRule {
                    name: name.clone(),
                    install_id: hex(&world.install.bytes()),
                    matches: world.matches.clone(),
                    bootstrap: world.bootstrap.to_string(),
                })
                .collect(),
            external: self.external.clone(),
            extension_origins: self.pages.keys().cloned().collect(),
            extension_bootstrap: self.extension_bootstrap.to_string(),
            observe_requests: self.observe_requests,
            observe_forms: self.observe_forms,
        }
    }
}

fn bootstrap(source: &str) -> Result<(), PlanError> {
    if source.len() > MAX_BOOTSTRAP_BYTES || source.contains('\0') {
        Err(PlanError::InvalidBootstrap)
    } else {
        Ok(())
    }
}

/// The module's renderer-side pre-filter, from the runtime's limits. The
/// runtime enforces the same limits authoritatively on every admitted post.
pub(crate) fn limits(role: PageRole) -> Limits {
    let message = match role {
        PageRole::OrdinaryTab => MAX_MESSAGE_BYTES_IN_WEB_RENDERER,
        PageRole::ExtensionOwned => MAX_MESSAGE_BYTES_RUNTIME_OWNED,
    };
    Limits {
        max_message_bytes: message as u32,
        frame_inflight_bytes: MAX_IN_FLIGHT_BYTES_PER_CONTEXT as u32,
        page_inflight_bytes: PAGE_IN_FLIGHT_BYTES as u32,
        frame_outstanding: MAX_OUTSTANDING_CALLS_PER_CONTEXT as u16,
        page_outstanding: PAGE_OUTSTANDING,
        frame_messages_per_second: RATE_MESSAGES_PER_SECOND,
        page_messages_per_second: PAGE_MESSAGES_PER_SECOND,
        frame_burst: RATE_BURST as u16,
        page_burst: PAGE_BURST,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zephium_webext_runtime::context::UserScriptWorldId;

    fn install(n: u128) -> ExtensionInstallId {
        ExtensionInstallId::from(n)
    }

    fn grant(origins: &[&str]) -> WorldGrant {
        WorldGrant {
            install: install(10),
            world: InjectedWorld::Isolated,
            matches: origins
                .iter()
                .map(|origin| MatchRule {
                    matcher: zephium_webext_runtime::pattern::ContentScriptMatcher::new(
                        &[format!("{origin}/*")],
                        &[],
                        &[],
                        &[],
                        Default::default(),
                    )
                    .expect("matcher"),
                    all_frames: true,
                    file_access: false,
                })
                .collect(),
            bootstrap: "void 0".into(),
        }
    }

    fn id(value: &str) -> ExtensionId {
        ExtensionId::parse(value).expect("id")
    }

    #[test]
    fn world_names_are_the_engine_principal_names_and_fit_the_module() {
        let name = world_name(install(0xab), &InjectedWorld::Isolated);
        assert_eq!(
            name,
            "zephium-principal-extension-000000000000000000000000000000ab"
        );
        let user = world_name(
            install(0xab),
            &InjectedWorld::UserScript(UserScriptWorldId::parse("w").expect("id")),
        );
        assert!(user.starts_with("zephium-webext-us-000000000000000000000000000000ab-"));
        assert!(user.len() <= 128);
        let other = world_name(
            install(0xab),
            &InjectedWorld::UserScript(UserScriptWorldId::parse("x").expect("id")),
        );
        assert_ne!(user, other);
    }

    #[test]
    fn compiled_patterns_match_unlisted_origins_and_exclusions() {
        let rule = MatchRule {
            matcher: zephium_webext_runtime::pattern::ContentScriptMatcher::new(
                &["<all_urls>"],
                &["https://excluded.test/*"],
                &[],
                &[],
                Default::default(),
            )
            .expect("matcher"),
            all_frames: true,
            file_access: false,
        };
        let plan = AttachPlan {
            worlds: vec![WorldGrant {
                matches: vec![rule],
                ..grant(&["https://example.com"])
            }],
            ..Default::default()
        };
        let resolved =
            ResolvedPlan::resolve(&plan, ExtensionScheme::CHROME_EXTENSION, None).expect("valid");
        let name = world_name(install(10), &InjectedWorld::Isolated);
        let facts = |uri: &str| zephium_webkit_web_process_protocol::protocol::FrameInfo {
            frame_id: 1,
            document_seq: 1,
            uri: uri.into(),
            is_main: true,
            table_generation: 0,
            parent_frame_id: None,
            creator_uri: None,
            opaque: false,
        };
        assert!(resolved
            .admits_world(&name, &facts("https://unlisted.test/page"))
            .is_some());
        assert!(resolved
            .admits_world(&name, &facts("https://excluded.test/page"))
            .is_none());
    }

    #[test]
    fn plans_are_bounded_and_unambiguous() {
        let scheme = ExtensionScheme::CHROME_EXTENSION;
        let duplicate = AttachPlan {
            worlds: vec![grant(&["https://a.test"]), grant(&["https://b.test"])],
            ..AttachPlan::default()
        };
        assert_eq!(
            ResolvedPlan::resolve(&duplicate, scheme, None),
            Err(PlanError::DuplicateWorld)
        );
        let empty = AttachPlan {
            worlds: vec![grant(&[])],
            ..AttachPlan::default()
        };
        assert_eq!(
            ResolvedPlan::resolve(&empty, scheme, None),
            Err(PlanError::EmptyWorld)
        );
        let nul = AttachPlan {
            extension_bootstrap: "a\0b".into(),
            ..AttachPlan::default()
        };
        assert_eq!(
            ResolvedPlan::resolve(&nul, scheme, None),
            Err(PlanError::InvalidBootstrap)
        );
        let many = AttachPlan {
            worlds: (0..=MAX_WORLDS as u128)
                .map(|n| WorldGrant {
                    install: install(n),
                    ..grant(&["https://a.test"])
                })
                .collect(),
            ..AttachPlan::default()
        };
        assert_eq!(
            ResolvedPlan::resolve(&many, scheme, None),
            Err(PlanError::TooMany)
        );
        let large = AttachPlan {
            worlds: (0..5u128)
                .map(|n| WorldGrant {
                    install: install(n),
                    bootstrap: "x".repeat(MAX_BOOTSTRAP_BYTES).into(),
                    ..grant(&["https://a.test"])
                })
                .collect(),
            ..AttachPlan::default()
        };
        assert_eq!(
            ResolvedPlan::resolve(&large, scheme, None),
            Err(PlanError::TooLarge)
        );
    }

    #[test]
    fn extension_pages_need_the_chrome_scheme_and_owned_views_only_their_own() {
        let own = id("abcdefghijklmnopabcdefghijklmnop");
        let other = id("ponmlkjihgfedcbaponmlkjihgfedcba");
        let pages = |extension: &ExtensionId| AttachPlan {
            extension_pages: vec![ExtensionPages {
                extension: extension.clone(),
                sandboxed_paths: vec!["/sandbox.html".into()],
            }],
            ..AttachPlan::default()
        };
        assert_eq!(
            ResolvedPlan::resolve(&pages(&own), ExtensionScheme::ZEPHIUM_EXTENSION, None),
            Err(PlanError::UnsupportedScheme)
        );
        let resolved =
            ResolvedPlan::resolve(&pages(&own), ExtensionScheme::CHROME_EXTENSION, Some(&own))
                .expect("own pages");
        let origin = format!("chrome-extension://{}", own.as_str());
        assert!(resolved.admits_page(&origin).is_some());
        assert_eq!(
            ResolvedPlan::resolve(
                &pages(&other),
                ExtensionScheme::CHROME_EXTENSION,
                Some(&own)
            ),
            Err(PlanError::NotOwnPages)
        );
        let mut with_world = pages(&own);
        with_world.worlds.push(grant(&["https://a.test"]));
        assert_eq!(
            ResolvedPlan::resolve(&with_world, ExtensionScheme::CHROME_EXTENSION, Some(&own)),
            Err(PlanError::NotOwnPages)
        );
        let mut bad_path = pages(&own);
        bad_path.extension_pages[0].sandboxed_paths = vec!["sandbox.html".into()];
        assert_eq!(
            ResolvedPlan::resolve(&bad_path, ExtensionScheme::CHROME_EXTENSION, None),
            Err(PlanError::InvalidSandboxPath)
        );
    }

    #[test]
    fn module_limits_come_from_the_runtime_and_pass_the_module_floor() {
        let tab = limits(PageRole::OrdinaryTab);
        assert!(tab.validate(PageRole::OrdinaryTab));
        assert_eq!(
            u64::from(tab.max_message_bytes),
            MAX_MESSAGE_BYTES_IN_WEB_RENDERER
        );
        assert_eq!(
            u32::from(tab.frame_outstanding),
            MAX_OUTSTANDING_CALLS_PER_CONTEXT
        );
        assert_eq!(tab.frame_messages_per_second, RATE_MESSAGES_PER_SECOND);
        assert_eq!(u32::from(tab.frame_burst), RATE_BURST);
        let owned = limits(PageRole::ExtensionOwned);
        assert!(owned.validate(PageRole::ExtensionOwned));
        assert!(!owned.validate(PageRole::OrdinaryTab));
        assert_eq!(
            u64::from(owned.max_message_bytes),
            MAX_MESSAGE_BYTES_RUNTIME_OWNED
        );
    }
}
