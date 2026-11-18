use super::*;
use crate::api::web_navigation::BrowserFrame;
use crate::browser::{BrowserPort, BrowserRequest, BrowserSnapshot, ExtensionInfo};
use crate::context::tests::{background_sender, doc, profile};
use crate::dispatch::{Handled, Invocation};
use crate::pattern::DocumentOrigin;
use crate::permissions::{
    InstallPermissions, PermissionDeclaration, PermissionRequest, SiteAccess,
};
use serde_json::json;
use url::Url;
use zephium_core::extensions::{
    ExtensionBrowserSurface, ExtensionBrowserSurfaceGeneration, ExtensionBrowserTab,
    ExtensionBrowserWindow,
};
use zephium_webext::manifest::Manifest;

struct Host {
    snapshot: BrowserSnapshot,
    frames: Vec<BrowserFrame>,
}
impl BrowserPort for Host {
    fn snapshot(&self, _: &Sender) -> Result<BrowserSnapshot, ApiError> {
        Ok(self.snapshot.clone())
    }
    fn extension_info(&self, _: &Sender) -> Result<ExtensionInfo, ApiError> {
        Ok(ExtensionInfo::default())
    }
    fn request(&mut self, _: BrowserRequest, call: Invocation<'_>) -> Handled {
        call.reply(Err(ApiError::unknown_method()))
    }
    fn frames(&self, _: &Sender, _: ItemId) -> Result<Vec<BrowserFrame>, ApiError> {
        Ok(self.frames.clone())
    }
}

fn host() -> Host {
    let url = Url::parse("https://example.com/").unwrap();
    let tab = ItemId::from(7);
    let surface = ExtensionBrowserSurface::new(
        profile(),
        ExtensionBrowserSurfaceGeneration::INITIAL,
        Some(1),
        vec![ExtensionBrowserWindow::new(
            1,
            false,
            Some(tab),
            vec![ExtensionBrowserTab::from_snapshot(
                None,
                tab,
                true,
                "Fixture",
                Some(&url),
                false,
                false,
            )
            .unwrap()],
        )
        .unwrap()],
    )
    .unwrap();
    let snapshot = BrowserSnapshot::from_surface(
        &surface,
        |_| Some(11),
        |_| Some(1),
        |_| Some((doc(1), "main".into())),
    )
    .unwrap();
    let frame = |id, generation, url: Url| BrowserFrame {
        tab,
        frame: FrameId::new(id),
        document: doc(generation),
        document_id: if id == 0 {
            "main".into()
        } else {
            format!("child-{id}")
        },
        origin: DocumentOrigin::of_url(&url),
        url,
        parent_frame_id: (id != 0).then_some(FrameId::MAIN),
        parent_document_id: (id != 0).then(|| "main".into()),
        document_lifecycle: "active".into(),
        frame_type: if id == 0 {
            "outermost_frame".into()
        } else {
            "sub_frame".into()
        },
        error_occurred: false,
    };
    Host {
        snapshot,
        frames: vec![
            frame(0, 1, url),
            frame(3, 4, Url::parse("https://example.com/child").unwrap()),
            frame(5, 6, Url::parse("https://foreign.test/").unwrap()),
        ],
    }
}

fn permissions() -> InstallPermissions {
    let (declaration, issues) = PermissionDeclaration::from_manifest(&Manifest::from_value(
        json!({"host_permissions":["https://example.com/*"]}),
    ));
    assert!(issues.is_empty());
    InstallPermissions::new(
        declaration,
        &PermissionRequest::default(),
        SiteAccess::OnAllSites,
        false,
    )
}

#[test]
fn tab_broadcast_routes_only_current_authorized_documents() {
    let request = parse_request("tabs.sendMessage", &[json!(11), json!({"hello":true})]).unwrap();
    let resolved =
        resolve_tab_request(&host(), &background_sender(), &permissions(), request).unwrap();
    let MessageRequest::Send {
        target: MessageTarget::ResolvedTab { routes },
        ..
    } = resolved
    else {
        panic!("expected resolved target")
    };
    assert_eq!(
        routes,
        vec![
            MessageRoute::Tab {
                tab: ItemId::from(7),
                frame: Some(FrameId::MAIN),
                document: Some(doc(1))
            },
            MessageRoute::Tab {
                tab: ItemId::from(7),
                frame: Some(FrameId::new(3)),
                document: Some(doc(4))
            }
        ]
    );
}

#[test]
fn frame_and_document_constraints_intersect_and_foreign_hosts_are_denied() {
    for method in ["tabs.sendMessage", "tabs.connect"] {
        let request = |options| {
            if method == "tabs.sendMessage" {
                parse_request(method, &[json!(11), json!(null), options]).unwrap()
            } else {
                parse_request(method, &[json!(11), options]).unwrap()
            }
        };
        let valid = request(json!({"frameId":3,"documentId":"child-3"}));
        assert!(resolve_tab_request(&host(), &background_sender(), &permissions(), valid).is_ok());
        for options in [
            json!({"frameId":3,"documentId":"main"}),
            json!({"documentId":"obsolete"}),
            json!({"frameId":5}),
        ] {
            assert!(resolve_tab_request(
                &host(),
                &background_sender(),
                &permissions(),
                request(options)
            )
            .is_err());
        }
    }
}

#[test]
fn foreign_profiles_and_stale_frame_observations_cannot_route() {
    let request = parse_request("tabs.connect", &[json!(11)]).unwrap();
    let mut host = host();
    host.snapshot.profile = zephium_core::ids::ProfileId::from(999);
    assert!(
        resolve_tab_request(&host, &background_sender(), &permissions(), request.clone()).is_err()
    );
    host.snapshot.profile = profile();
    host.frames[0].document = doc(99);
    assert!(
        resolve_tab_request(&host, &background_sender(), &permissions(), request.clone()).is_err()
    );
    host.frames[0].document = doc(1);
    host.frames[1].tab = ItemId::from(999);
    assert!(resolve_tab_request(&host, &background_sender(), &permissions(), request).is_err());
}
