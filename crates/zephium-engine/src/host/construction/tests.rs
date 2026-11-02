use super::*;

#[test]
fn wry_document_start_selection_preserves_protected_order_and_frame_policy() {
    use zephium_core::ids::ScriptId;
    use zephium_core::injection::MatchSet;
    use zephium_core::ports::engine::ScriptOwner;

    let script = |id, run_at, all_frames| UserScript {
        id: ScriptId::from(id),
        owner: ScriptOwner::Builtin,
        source: format!("/* {id} */").into(),
        world: World::Page,
        matches: MatchSet::all_urls(),
        run_at,
        all_frames,
    };
    let scripts = vec![
        script(1, RunAt::DocumentStart, false),
        script(2, RunAt::DocumentStart, false),
        script(3, RunAt::DocumentStart, true),
        script(4, RunAt::DocumentEnd, false),
    ];

    let selected = wry_document_start_scripts(&scripts).collect::<Vec<_>>();
    assert_eq!(
        selected.iter().map(|script| script.id).collect::<Vec<_>>(),
        vec![ScriptId::from(1), ScriptId::from(2), ScriptId::from(3)]
    );
    assert!(!selected[0].all_frames);
    assert!(!selected[1].all_frames);
    assert!(selected[2].all_frames);
}

#[test]
fn windows_construction_settlement_rejects_every_unproven_native_result() {
    let proven = WindowsConstructionSettlement {
        built_view_exists: true,
        native_cleanup_debts: 0,
        native_cleanup_overflowed: false,
        host_cleanup_invariant_failed: false,
        native_accounting_failed: false,
        profile_is_quarantined: false,
    };
    assert!(proven.admits_view());

    for rejected in [
        WindowsConstructionSettlement {
            built_view_exists: false,
            ..proven
        },
        WindowsConstructionSettlement {
            native_cleanup_debts: 1,
            ..proven
        },
        WindowsConstructionSettlement {
            native_cleanup_overflowed: true,
            ..proven
        },
        WindowsConstructionSettlement {
            host_cleanup_invariant_failed: true,
            ..proven
        },
        WindowsConstructionSettlement {
            native_accounting_failed: true,
            ..proven
        },
        WindowsConstructionSettlement {
            profile_is_quarantined: true,
            ..proven
        },
    ] {
        assert!(!rejected.admits_view());
    }
}

#[test]
fn windows_cleanup_barrier_is_collected_before_warm_spare_adoption() {
    let source = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/host/construction.rs"
    ));
    let create_view = source.find("pub(crate) fn create_view(").unwrap();
    let source = &source[create_view..];
    let collection = source
        .find("self.collect_pending_windows_cleanup_debts();")
        .unwrap();
    let spare_adoption = source
        .find("if let Some(mut spare) = self.spare.take_if")
        .unwrap();
    assert!(collection < spare_adoption);
}

#[test]
fn rejected_windows_construction_transfers_lease_before_drop_and_recollection() {
    let source = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/host/construction.rs"
    ));
    let branch = source
        .split_once("if built.is_some() && !settlement.admits_view()")
        .expect("construction lost the terminal settlement branch")
        .1;
    let lease_transfer = branch
        .find("view.native_resource = native_resource.take();")
        .expect("rejected view lost its exact lease transfer");
    let local_drop = branch
        .find("drop(built.take());")
        .expect("rejected native view is no longer dropped");
    let debt_import = branch
        .find("self.collect_pending_windows_cleanup_debts();")
        .expect("rejected native view debt is no longer reimported");
    let terminal_return = branch
        .find("return None;")
        .expect("rejected native view can escape its terminal branch");
    assert!(lease_transfer < local_drop);
    assert!(local_drop < debt_import);
    assert!(debt_import < terminal_return);
}

#[test]
fn explicit_content_policy_brackets_every_first_native_navigation() {
    let file = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/host/construction.rs"
    ));
    let source = file
        .split_once("fn build_view_inner(")
        .expect("construction lost build_view_inner")
        .1
        .split_once("/// Why a profile's WebView2 environment could not be established.")
        .expect("construction lost build_view_inner's end")
        .0;
    let policy_gate = source
        .find("let Some(content_policy) = self.applied_content_policy")
        .expect("construction lost the explicit profile-policy gate");
    let builder = source
        .find("WebViewBuilder::new")
        .expect("construction lost its native builder");
    assert!(
        policy_gate < builder,
        "policy absence must reject before allocating a native view"
    );

    let last_native_hardening = source
        .rfind("crate::platform::imp::configure(")
        .expect("construction lost platform hardening");
    let policy_install = source
        .find("install_scoped_content_policy_on_view(")
        .expect("construction lost native content-policy installation");
    let first_load = source
        .find("view.load_url(url)")
        .expect("construction lost its explicit first navigation");
    assert!(
        last_native_hardening < policy_install && policy_install < first_load,
        "native policy must install after hardening and before first navigation"
    );
}

#[test]
fn only_tabs_a_person_reads_sign_in_or_get_certificate_exceptions() {
    let file = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/host/construction.rs"
    ));
    let (tabs, rest) = file
        .split_once("fn build_view_inner(")
        .expect("construction lost build_view_inner")
        .1
        .split_once("/// Why a profile's WebView2 environment could not be established.")
        .expect("construction lost build_view_inner's end");
    for routed in [
        ".with_http_auth_handler(",
        ".route_basic_authentication(",
        ".with_server_trust_exceptions(",
        ".route_certificate_errors(",
    ] {
        assert!(tabs.contains(routed), "tabs lost {routed}");
    }
    // Agent, extension and bootstrap views keep cancelling every challenge.
    for (name, source) in [
        ("profile bootstrap", rest),
        (
            "macOS agent",
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/src/platform/macos/agent_context.rs"
            )),
        ),
        (
            "Windows agent",
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/src/platform/windows/agent_context.rs"
            )),
        ),
        (
            "macOS extension",
            include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/host/webext.rs")),
        ),
        (
            "Windows extension",
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/src/host/webext_windows.rs"
            )),
        ),
        (
            "Windows extension popup",
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/src/host/webext_windows_popup.rs"
            )),
        ),
    ] {
        assert!(
            !source.contains("with_http_auth_handler")
                && !source.contains("route_basic_authentication"),
            "{name} views must never route a sign-in challenge"
        );
        assert!(
            !source.contains("with_server_trust_exceptions")
                && !source.contains("route_certificate_errors")
                && !source.contains("certificate_exceptions"),
            "{name} views must never get a certificate exception"
        );
    }
}
