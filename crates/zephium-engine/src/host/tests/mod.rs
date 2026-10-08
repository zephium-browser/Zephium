fn raw_view_construction_policy() -> &'static str {
    include_str!("../construction.rs")
        .split_once("let mut builder = builder")
        .expect("raw view policy builder")
        .1
        .split_once("/// Why a profile's WebView2 environment could not be established.")
        .expect("agent environment bootstrap boundary")
        .0
}

#[test]
fn protected_document_start_scripts_flow_through_the_ordered_builder_path() {
    let source = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/host/construction.rs"
    ));
    assert_eq!(
        source
            .matches("for script in wry_document_start_scripts(&scripts)")
            .count(),
        1
    );
    assert!(source.contains("builder.with_initialization_script_for_main_only("));
    assert!(source.contains("script.source.as_ref(),"));
    assert!(source.contains("!script.all_frames,"));
}

#[test]
fn raw_popups_use_owned_native_adoption_with_explicit_gesture_and_scope_checks() {
    let raw_policy = raw_view_construction_policy();
    assert!(raw_policy.contains("with_new_window_req_handler"));
    assert!(raw_policy.contains("!features.user_initiated"));
    let boundary = include_str!("../page_open.rs");
    assert!(boundary.contains("presentation_permit.load(Ordering::Acquire)"));
    assert!(boundary.contains("same_generation(permit)"));
    assert!(boundary.contains("matches_activity(activity)"));
    assert!(boundary
        .split_whitespace()
        .collect::<String>()
        .contains("native_open_authority.reserve"));
    assert!(boundary.contains("NativeViewPurpose::NativeTab(features.opener)"));
}

#[test]
fn both_successful_view_insertion_paths_reconcile_retained_layouts() {
    // Keep the construction-to-stage handoff explicit across module boundaries.
    let construction = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/host/construction.rs"
    ));
    let stages = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/host/stages.rs"));
    let create_view = construction
        .split("pub(crate) fn create_view(")
        .nth(1)
        .expect("view creation implementation")
        .split("// Rebuilt after adoption")
        .next()
        .expect("bounded view creation implementation");

    // One call follows warm-spare adoption and one follows a fresh native
    // build. Keeping both prevents latest-layout coalescing from stranding
    // either construction path behind an earlier queued layout task.
    assert_eq!(
        create_view
            .matches("self.finish_new_view_insertion")
            .count(),
        2
    );
    assert!(stages.contains("filter(|stage| stage.contains_item(id))"));
    assert!(stages.contains("Stage::exclude_unstaged(view)"));
}

#[test]
fn retained_layouts_accept_reserved_views_awaiting_native_construction() {
    let stages = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/host/stages.rs"));
    let macos = stages
        .split("#[cfg(target_os = \"macos\")]\n    fn apply_content(")
        .nth(1)
        .expect("macOS host layout")
        .split("#[cfg(target_os = \"macos\")]\n    pub(crate) fn set_drop_indicator")
        .next()
        .expect("bounded macOS host layout");
    let other = stages
        .split("#[cfg(not(target_os = \"macos\"))]\n    fn apply_content(")
        .nth(1)
        .expect("Windows/Linux host layout")
        .split("#[cfg(not(target_os = \"macos\"))]\n    pub(crate) fn set_drop_indicator")
        .next()
        .expect("bounded Windows/Linux host layout");

    for layout in [macos, other] {
        let pending_construction = layout
            .split("let Some(view) = self.views.get(id) else {")
            .nth(1)
            .expect("native view lookup")
            .split("};")
            .next()
            .expect("missing-view branch");
        assert!(pending_construction.contains("continue;"));
        assert!(!pending_construction.contains("return false;"));
    }
}

#[test]
fn windows_superseded_native_placement_requeues_without_spending_failure_budget() {
    let source = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/platform/windows/stage.rs"
    ));
    let application = source
        .split("fn apply_native_placement(")
        .nth(1)
        .expect("Windows placement application")
        .split("fn conceal_superseded_placement")
        .next()
        .expect("bounded Windows placement application");
    for (start, end, primitive) in [
        (
            "if placement.delta.container_rect",
            "if placement.delta.rounded",
            "SetWindowPos(",
        ),
        (
            "if placement.delta.rounded",
            "if placement.delta.controller_size",
            "SetWindowRgn(",
        ),
        (
            "if placement.delta.controller_size",
            "if placement.delta.notify_parent_position",
            "SetBounds(",
        ),
        (
            "if placement.delta.notify_parent_position",
            "// COM can re-enter the message pump",
            "NotifyParentWindowPositionChanged()",
        ),
    ] {
        let boundary = application
            .split(start)
            .nth(1)
            .expect("native geometry boundary")
            .split(end)
            .next()
            .expect("bounded native geometry boundary");
        assert!(
            boundary.find(primitive).unwrap() < boundary.find("if !placement_is_current").unwrap()
        );
        assert!(boundary.contains("conceal_superseded_placement"));
    }
    let conceal = source
        .split("fn conceal_superseded_placement")
        .nth(1)
        .expect("superseded geometry concealment")
        .split("fn finish_native_placement")
        .next()
        .expect("bounded superseded geometry concealment");
    assert!(conceal.contains("SW_HIDE"));
    assert!(conceal.contains("SetIsVisible(false)"));
    assert!(conceal.contains("finish_native_placement(state, placement, applied, false)"));

    let settlement = source
        .split("fn finish_native_placement(")
        .nth(1)
        .expect("Windows placement settlement")
        .split("fn draw_indicator(")
        .next()
        .expect("bounded Windows placement settlement");

    assert!(settlement.contains("state.revision != placement.revision"));
    assert!(settlement.contains("if superseded || failed"));
    let superseded = settlement
        .split("if superseded {")
        .nth(1)
        .expect("superseded placement branch")
        .split("} else if failed {")
        .next()
        .expect("bounded superseded placement branch");
    assert!(superseded.contains("retry = true"));
    assert!(!superseded.contains("consecutive_failures"));
}

#[test]
fn windows_reentrant_hide_preserves_newer_cache_and_forces_an_exact_redrive() {
    let source = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/platform/windows/stage.rs"
    ));
    let hide = source
        .split("fn hide_views(")
        .nth(1)
        .expect("Windows immediate hide")
        .split("fn hide_is_current")
        .next()
        .expect("bounded Windows immediate hide");
    assert!(hide.contains("revision: state.revision"));
    for primitive in ["ShowWindow(hide.container, SW_HIDE)", "SetIsVisible(false)"] {
        let boundary = hide.find(primitive).expect("native hide boundary");
        assert!(hide[boundary..].contains("hide_is_current("));
        assert!(hide[boundary..].contains("redrive_uncertain_visibility("));
    }

    let settlement = source
        .split("fn finish_native_placement(")
        .nth(1)
        .expect("Windows placement settlement")
        .split("fn draw_indicator(")
        .next()
        .expect("bounded Windows placement settlement");
    let superseded = settlement
        .split("if superseded {")
        .nth(1)
        .expect("superseded cache branch")
        .split("} else if failed {")
        .next()
        .expect("bounded superseded cache branch");
    assert!(!superseded.contains("view.applied.set"));
    assert!(settlement.contains("state.visibility_uncertain.insert(placement.id)"));
}

#[test]
fn collapsed_split_leaves_hide_without_invalid_native_bounds_and_reappear_on_resize() {
    // The shared pure helper has collapse/grow unit coverage. These
    // platform-boundary assertions ensure every stage uses that decision
    // before its native geometry/reveal primitive.
    let windows = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/platform/windows/stage.rs"
    ));
    let windows_layout = windows
        .split("let pane_rects = tree")
        .nth(1)
        .expect("Windows pane geometry")
        .split("let mut parent_screen")
        .next()
        .expect("bounded Windows pane geometry");
    assert!(windows_layout.contains(".filter_map"));
    assert!(windows_layout.contains("rounded_native_size"));
    assert!(!windows_layout.contains(".max(0)"));

    let linux = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/platform/linux/stage.rs"
    ));
    let linux_placement = linux
        .split("let placements = views")
        .nth(1)
        .expect("Linux pane placement")
        .split("// Fail closed")
        .next()
        .expect("bounded Linux pane placement");
    assert!(linux_placement.contains("rounded_native_size"));
    assert!(linux_placement.contains("pane.is_some()"));
    let linux_geometry = linux
        .split("// Keep provisional current-layout widgets mapped")
        .nth(1)
        .expect("Linux native geometry")
        .split("fn revision_is_current")
        .next()
        .expect("bounded Linux native geometry");
    assert!(!linux_geometry.contains("max(1.0)"));
    assert!(linux_geometry.contains("fixed.move_(&view.view, parked_x, 0)"));
    assert!(linux_geometry.contains("view.view.set_size_request"));

    let mac = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/platform/macos/stage.rs"
    ));
    let resize = mac
        .split("fn resize_subviews")
        .nth(1)
        .expect("macOS resize callback")
        .split("fn mouse_down")
        .next()
        .expect("bounded macOS resize callback");
    assert!(resize.contains("self.bump_layout_epoch()"));
    assert!(resize.contains("self.sync_visibility()"));
    let visibility = mac
        .split("fn sync_visibility(&self)")
        .nth(1)
        .expect("macOS visibility pass")
        .split("fn defer_stage_retry")
        .next()
        .expect("bounded macOS visibility pass");
    assert!(visibility.contains("paintable.contains(id)"));
}

#[test]
fn terminal_native_stage_failures_have_exact_retirement_and_mandatory_fatal_handoff() {
    let stages = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/host/stages.rs"));
    let mac_failure = stages
        .split("fn on_macos_stage_failure(")
        .nth(1)
        .expect("macOS terminal stage handler")
        .split("fn ensure_stage(")
        .next()
        .expect("bounded macOS terminal stage handler");
    assert!(mac_failure.contains("Retained::as_ptr(stage) as usize != failed_identity"));
    assert!(
        mac_failure.find("attached_items()").unwrap() < mac_failure.find("stages.remove").unwrap()
    );
    assert!(mac_failure.contains("self.native_terminal_failure"));

    let mac_stage = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/platform/macos/stage.rs"
    ));
    assert!(mac_stage.contains("pub fn attached_items(&self) -> Option<Vec<ItemId>>"));
    let container = mac_stage
        .split("fn sync_container_visibility(&self)")
        .nth(1)
        .expect("macOS container reveal")
        .split("fn bump_layout_epoch")
        .next()
        .expect("bounded macOS container reveal");
    let reveal = container.find("self.setHidden(false)").unwrap();
    assert!(container[..reveal].contains("stage_retry_terminal"));
    assert!(container[reveal..].contains("stage_retry_terminal"));

    for reason in [
        "terminal Windows stage failure was not admitted by the engine host",
        "terminal macOS stage failure was not admitted by the engine host",
    ] {
        let failure = stages.find(reason).expect("terminal admission handoff");
        assert!(stages[..failure].rfind("if !admitted").is_some());
        assert!(stages[..failure].rfind("native_terminal_failure").is_some());
    }
}

#[test]
fn macos_divider_capture_survives_geometry_only_relayout_but_not_topology_change() {
    let source = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/platform/macos/stage.rs"
    ));
    let set_tree = source
        .split("pub fn set_tree(&self, tree: Option<Pane>)")
        .nth(1)
        .expect("macOS stage tree setter")
        .split("pub fn set_on_ratio")
        .next()
        .expect("bounded macOS stage tree setter");

    // A drag only moves a guide; the installed tree is untouched until
    // mouseUp, so a geometry-only relayout cannot snap a drag back and only a
    // topology change revokes the capture.
    assert!(set_tree.contains("!current.same_topology(next)"));
    assert_eq!(set_tree.matches("cancel_split_drag").count(), 1);
    let topology_change = set_tree
        .split("if topology_changed {")
        .nth(1)
        .expect("topology-change capture revocation");
    assert!(topology_change
        .trim_start()
        .starts_with("self.cancel_split_drag(false);"));
    let dragged = source
        .split("fn mouse_dragged(&self, event: &NSEvent)")
        .nth(1)
        .expect("macOS divider drag handler")
        .split("fn mouse_up(&self, event: &NSEvent)")
        .next()
        .expect("bounded divider drag handler");
    assert!(!dragged.contains("tree.try_borrow_mut"));
    assert!(!dragged.contains("on_ratio"));

    let begin_update = source
        .split("pub fn begin_content_update(&self, visible: bool)")
        .nth(1)
        .expect("macOS stage content-update reservation")
        .split("pub fn content_update_is_current")
        .next()
        .expect("bounded content-update reservation");
    let hidden = begin_update
        .split("if !visible")
        .nth(1)
        .expect("hidden-stage capture revocation");
    assert!(hidden.contains("self.cancel_split_drag(false)"));
}

#[test]
fn linux_stage_exhaustion_retains_one_coalesced_idle_redrive() {
    let source = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/platform/linux/stage.rs"
    ));
    let sync = source
        .split("fn sync(")
        .nth(1)
        .expect("Linux stage sync")
        .split("fn schedule_sync")
        .next()
        .expect("bounded Linux stage sync");
    assert!(sync.matches("schedule_sync(state, sync_scheduled)").count() >= 2);
    assert!(sync.contains("conceal_views_until_retry(state)"));

    let retry = source
        .split("fn schedule_sync")
        .nth(1)
        .expect("Linux stage retry driver")
        .split("fn make_indicator")
        .next()
        .expect("bounded Linux stage retry driver");
    assert!(retry.contains("sync_scheduled.replace(true)"));
    assert!(retry.contains("glib::idle_add_local_once"));
    assert!(retry.contains("Rc::downgrade(state)"));
    assert!(retry.contains("sync_scheduled.set(false)"));
    assert!(retry.contains("sync(&state, &sync_scheduled)"));
    assert!(retry.contains("revoke_and_unmap_for_retry(state, revision, &view.view)"));

    let fail_closed = source
        .split("fn revoke_and_unmap_for_retry")
        .nth(1)
        .expect("Linux fail-closed retry barrier")
        .split("fn view_may_reveal")
        .next()
        .expect("bounded Linux fail-closed retry barrier");
    assert!(fail_closed.contains("view.set_sensitive(false)"));
    assert!(fail_closed.contains("view.set_opacity(0.0)"));
    assert_eq!(
        fail_closed.matches("view.set_child_visible(false)").count(),
        2
    );
}

#[test]
fn raw_native_views_never_request_focus_during_construction() {
    let raw_policy = raw_view_construction_policy();
    assert_eq!(raw_policy.matches(".with_focused(false)").count(), 1);
}

#[cfg(target_os = "macos")]
#[test]
fn hidden_wkwebview_construction_preserves_responder_and_cannot_activate() {
    let source = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../vendor/wry/src/wkwebview/mod.rs"
    ));
    let constructor = source
        .split("fn new_ns_view(")
        .nth(1)
        .and_then(|source| source.split("  pub fn id(&self)").next())
        .expect("bounded WKWebView constructor");
    let policy = constructor
        .find("let focuses_during_initial_construction =")
        .expect("initial focus policy capture");
    let responder_capture = constructor
        .find("let unfocused_child_focus =")
        .expect("unfocused child responder capture");
    let parenting = constructor
        .find("ns_view.addSubview(&webview)")
        .expect("child WebView parenting");
    let responder_settlement = constructor
        .find("preserve_unfocused_child_focus(snapshot")
        .expect("same-stack responder settlement");
    let responder_rollback = constructor
        .find("rollback_unfocused_child_attachment(snapshot")
        .expect("failed focus settlement rollback");
    let error_return = constructor
        .find("return Err(error)")
        .expect("constructor failure after rollback");
    let gate = constructor
        .find("if focuses_during_initial_construction {")
        .expect("application activation focus gate");
    let activation = constructor
        .find("NSApplication::activate(&app)")
        .expect("application activation");

    assert!(policy < responder_capture);
    assert!(responder_capture < parenting);
    assert!(parenting < responder_settlement);
    assert!(responder_settlement < responder_rollback);
    assert!(responder_rollback < error_return);
    assert!(error_return < gate);
    assert!(gate < activation);
    assert_eq!(
        constructor.matches("NSApplication::activate(&app)").count(),
        1
    );
    assert_eq!(
        constructor
            .matches("NSApplication::activateIgnoringOtherApps(&app, true)")
            .count(),
        1
    );
}

#[cfg(target_os = "macos")]
#[test]
fn hidden_wkwebviews_cannot_become_first_responder_after_construction() {
    let source = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../vendor/wry/src/wkwebview/class/wry_web_view.rs"
    ));
    for method in ["fn accepts_first_responder", "fn become_first_responder"] {
        let body = source
            .split(method)
            .nth(1)
            .and_then(|source| source.split("    #[").next())
            .expect("bounded responder override");
        let hidden = body.find("if self.isHidden()").expect("hidden-state gate");
        let refusal = body.find("Bool::NO").expect("hidden focus refusal");
        let native = body.find("super(self)").expect("visible native behavior");
        assert!(hidden < refusal);
        assert!(refusal < native);
    }
}

#[test]
fn native_completion_waits_for_shell_ordered_presentation_acknowledgement() {
    let navigation = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/host/navigation.rs"
    ));
    let permits = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/host/permits.rs"));
    let completion = permits
        .split("fn queue_navigation_completion(")
        .nth(1)
        .expect("navigation completion queue")
        .split("fn queue_navigation_failure(")
        .next()
        .expect("bounded navigation completion queue");
    assert!(completion.contains("emit_navigation_observation"));
    assert!(completion.contains("emit_navigation_ready"));
    assert!(!completion.contains("present_navigation_epoch"));

    let ready = navigation
        .split("fn emit_navigation_ready(")
        .nth(1)
        .expect("navigation-ready emitter")
        .split("fn present_navigation_epoch(")
        .next()
        .expect("bounded navigation-ready emitter");
    assert!(ready.contains("EngineEvent::PresentationReady"));
}

#[test]
fn every_identity_bearing_commit_rearms_presentation_but_history_observation_does_not() {
    let host = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/host/construction.rs"
    ));
    let permits = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/host/permits.rs"));
    let native_handler = host
        .split("builder = builder.with_navigation_event_handler")
        .nth(1)
        .expect("raw navigation identity handler")
        .split("#[cfg(target_os = \"windows\")]")
        .next()
        .expect("bounded raw navigation identity handler");
    let committed = native_handler
        .split("NavigationTransition::Committed(epoch)")
        .nth(1)
        .expect("identity-bearing commit branch")
        .split("NavigationTransition::Finished(epoch)")
        .next()
        .expect("bounded identity-bearing commit branch");
    assert!(committed.contains("queue_navigation_commit"));

    let commit_queue = permits
        .split("fn queue_navigation_commit(")
        .nth(1)
        .expect("commit presentation queue")
        .split("fn queue_navigation_completion(")
        .next()
        .expect("bounded commit presentation queue");
    assert!(commit_queue.contains("rearm_navigation_presentation"));
    assert!(commit_queue.contains("emit_navigation_observation"));

    let source_observer = host
        .split("let observer = match crate::platform::imp::install_navigation_observer")
        .nth(1)
        .expect("same-document source observer")
        .split("if !event_permit.allows_navigation(url)")
        .next()
        .expect("bounded same-document source observer");
    assert!(source_observer.contains("emit_navigation_observation"));
    assert!(!source_observer.contains("rearm_navigation_presentation"));

    // Reload, history traversal, explicit navigation and page-driven
    // navigation all converge on the same native Committed transition.
    // The Wry guard hides before callback admission on all desktop ports.
    let raw_policy = raw_view_construction_policy();
    assert!(raw_policy.contains("with_navigation_presentation_guard(move ||"));
    assert!(raw_policy.contains("guard_presentation_permit.store(false, Ordering::Release)"));
    let webview2 = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../vendor/wry/src/webview2/mod.rs"
    ));
    assert!(webview2.contains("navigation_presentation_guard"));
    assert!(webview2.contains("ShowWindow(hwnd, SW_HIDE)"));
    assert!(webview2.contains("committed_controller.SetIsVisible(false)"));
    let guarded_webview2 = webview2
        .split("if let Some(guard) = navigation_presentation_guard.as_ref()")
        .nth(1)
        .expect("WebView2 commit guard");
    assert!(
        guarded_webview2.find("guard();").unwrap()
            < guarded_webview2.find("ShowWindow(hwnd, SW_HIDE)").unwrap()
    );

    let webkitgtk = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../vendor/wry/src/webkitgtk/mod.rs"
    ));
    let guarded_gtk = webkitgtk
        .split("if native_committed {")
        .nth(1)
        .and_then(|source| source.split("// Legacy page load handler").next())
        .expect("bounded WebKitGTK commit guard");
    let gtk_guard = guarded_gtk.find("guard();").unwrap();
    let gtk_input = guarded_gtk.find("webview.set_sensitive(false)").unwrap();
    let gtk_paint = guarded_gtk.find("webview.set_opacity(0.0)").unwrap();
    assert!(gtk_guard < gtk_input);
    assert!(gtk_input < gtk_paint);

    let wkwebview = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../vendor/wry/src/wkwebview/navigation.rs"
    ));
    let guarded_wk = wkwebview
        .split("if let Some(guard) = &this.ivars().navigation_presentation_guard")
        .nth(1)
        .expect("WKWebView commit guard");
    assert!(
        guarded_wk.find("guard();").unwrap() < guarded_wk.find("webview.setHidden(true)").unwrap()
    );
}

#[test]
fn every_native_stage_revalidates_the_generation_permit_around_reveal() {
    let mac = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/platform/macos/stage.rs"
    ));
    let mac_sync = mac
        .split("fn sync_visibility(&self)")
        .nth(1)
        .expect("macOS visibility sync")
        .split("fn bump_layout_epoch")
        .next()
        .expect("bounded macOS visibility sync");
    let mac_reveal = mac_sync
        .find("view.view.setHidden(false)")
        .expect("macOS reveal primitive");
    assert!(mac_sync[..mac_reveal]
        .rfind("presentation_permit.load(Ordering::Acquire)")
        .is_some());
    assert!(mac_sync[mac_reveal..]
        .find("presentation_permit.load(Ordering::Acquire)")
        .is_some());

    let linux = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/platform/linux/stage.rs"
    ));
    let linux_sync = linux
        .split("fn sync(")
        .nth(1)
        .expect("Linux stage sync")
        .split("fn schedule_sync")
        .next()
        .expect("bounded Linux stage sync");
    let linux_reveal = linux_sync
        .find("view.view.set_opacity(1.0)")
        .expect("Linux reveal primitive");
    assert!(linux_sync[..linux_reveal]
        .rfind("view_may_reveal(state, revision, *id, view)")
        .is_some());
    assert!(linux_sync[linux_reveal..]
        .find("view_may_reveal(state, revision, *id, view)")
        .is_some());
    let linux_input = linux_sync
        .rfind("view.view.set_sensitive(true)")
        .expect("Linux input reveal primitive");
    assert!(linux_reveal < linux_input);
    assert!(linux_sync[..linux_input]
        .rfind("view_may_reveal(state, revision, *id, view)")
        .is_some());
    assert!(linux_sync[linux_input..]
        .find("view_may_reveal(state, revision, *id, view)")
        .is_some());

    let windows = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/platform/windows/stage.rs"
    ));
    let windows_reveal = windows
        .split("fn apply_native_placement(")
        .nth(1)
        .expect("Windows native placement")
        .split("fn finish_native_placement")
        .next()
        .expect("bounded Windows native placement");
    let controller_reveal = windows_reveal
        .find("placement.controller.SetIsVisible(true)")
        .expect("WebView2 controller reveal");
    assert!(windows_reveal[..controller_reveal]
        .rfind("placement_may_reveal(state, placement)")
        .is_some());
    assert!(windows_reveal[controller_reveal..]
        .find("placement_may_reveal(state, placement)")
        .is_some());
    let window_reveal = windows_reveal
        .find("ShowWindow(placement.container, SW_SHOWNA)")
        .expect("WebView2 child window reveal");
    assert!(windows_reveal[..window_reveal]
        .rfind("placement_may_reveal(state, placement)")
        .is_some());
    assert!(windows_reveal[window_reveal..]
        .find("placement_may_reveal(state, placement)")
        .is_some());

    let mac_container = mac
        .split("fn sync_container_visibility(&self)")
        .nth(1)
        .expect("macOS container visibility sync")
        .split("fn bump_layout_epoch")
        .next()
        .expect("bounded macOS container visibility sync");
    let container_reveal = mac_container
        .find("self.setHidden(false)")
        .expect("macOS stage-container reveal");
    assert!(mac_container[..container_reveal]
        .rfind("content_update_epoch.get() == epoch")
        .is_some());
    assert!(mac_container[container_reveal..]
        .find("content_update_epoch.get() != epoch")
        .is_some());

    let mac_host_layout = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/host/stages.rs"))
        .split("#[cfg(target_os = \"macos\")]\n    fn apply_content(")
        .nth(1)
        .expect("macOS host layout")
        .split("#[cfg(target_os = \"macos\")]\n    pub(crate) fn set_drop_indicator")
        .next()
        .expect("bounded macOS host layout");
    assert!(
        mac_host_layout.find("begin_content_update").unwrap()
            < mac_host_layout.find("stage_set_frame").unwrap()
    );
    assert!(mac_host_layout.contains("content_update_is_current"));
    assert!(mac_host_layout.contains("finish_content_update"));
    assert!(!mac_host_layout.contains("stage.setHidden(false)"));

    let stages = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/host/stages.rs"));
    assert!(
        stages
            .matches("view.presentation_permit.load(Ordering::Acquire)")
            .count()
            >= 5
    );
    assert!(stages.matches("view.presentable").count() >= 5);
}

#[test]
fn title_callbacks_are_quarantined_until_exact_finished_document_attribution() {
    let host = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/host/construction.rs"
    ));
    let navigation = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/host/navigation.rs"
    ));
    let permits = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/host/permits.rs"));
    let title_callback = host
        .split(".with_document_title_changed_handler")
        .nth(1)
        .expect("raw title callback")
        .split("// Intentionally do not install a new-window callback")
        .next()
        .expect("bounded raw title callback");
    assert!(title_callback.contains("with_title_observation"));
    assert!(!title_callback.contains("title_permit.emit"));

    let completion = permits
        .split("fn queue_navigation_completion(")
        .nth(1)
        .expect("navigation completion queue")
        .split("fn queue_navigation_failure(")
        .next()
        .expect("bounded navigation completion queue");
    let title = completion
        .find("complete_title_attribution")
        .expect("finished native title query");
    let ready = completion
        .find("emit_navigation_ready")
        .expect("presentation-ready event");
    assert!(title < ready);

    let observed = navigation
        .split("fn emit_title_observation(")
        .nth(1)
        .expect("title observation gate")
        .split("fn emit_navigation_observation(")
        .next()
        .expect("bounded title observation gate");
    assert!(observed.contains("view.presentable"));
    assert!(observed.contains("view.title_ready != Some(epoch)"));
    assert!(observed.contains(".document_title()"));
}

#[test]
fn user_native_action_results_are_never_silently_discarded() {
    let navigation = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/host/navigation.rs"
    ));
    let page_ops = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/host/page_ops.rs"));
    let actions = navigation
        .split("fn invoke_navigation_action(")
        .nth(1)
        .expect("native navigation-action adapter")
        .split("\n}\n\n#[cfg(test)]")
        .next()
        .expect("bounded native navigation-action adapter");
    assert!(actions.contains("EngineEvent::NativeActionFailed"));
    assert!(actions.contains("emit_navigation_observation"));
    assert!(!actions.contains("let _ = view.reload()"));
    assert!(!actions.contains("let _ = view.go_back()"));
    assert!(!actions.contains("let _ = view.go_forward()"));

    let zoom = page_ops
        .split("pub(crate) fn zoom(")
        .nth(1)
        .expect("native zoom adapter")
        .split("pub(crate) fn extract_html(")
        .next()
        .expect("bounded native zoom adapter");
    assert!(zoom.contains("EngineEvent::ZoomSettled"));
    assert!(zoom.contains("settled_zoom_scale"));
    assert!(!zoom.contains("let _ = view.zoom"));
}

#[test]
fn raw_native_media_surfaces_are_deny_only_or_exactly_brokered_per_view() {
    let source = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/host/construction.rs"
    ));
    let raw_policy = raw_view_construction_policy();
    assert!(raw_policy.contains("with_fullscreen_enabled(true)"));
    // Picture-in-picture is a per-view grant for tabs a person reads, never
    // inherited from the chrome's compiled features.
    assert!(raw_policy.contains("with_picture_in_picture_enabled(true)"));
    assert!(raw_policy.contains("with_permission_handler(move |kind|"));
    assert!(raw_policy.contains("permission_presentation.load(Ordering::Acquire)"));
    assert!(raw_policy.contains("permission_window_is_foreground(permission_window)"));
    assert!(source.contains("with_permission_request_handler(move |request|"));
    assert!(source.contains("page_permissions::admit_native_request("));

    use super::construction::raw_content_permission;
    use wry::{PermissionKind, PermissionResponse};
    let media = if cfg!(windows) {
        PermissionResponse::Prompt
    } else {
        PermissionResponse::Deny
    };
    assert_eq!(raw_content_permission(PermissionKind::Camera), media);
    assert_eq!(raw_content_permission(PermissionKind::Microphone), media);
    for kind in [
        PermissionKind::DisplayCapture,
        PermissionKind::Geolocation,
        PermissionKind::Notifications,
        PermissionKind::ClipboardRead,
        PermissionKind::Sensors,
        PermissionKind::Other,
    ] {
        assert_eq!(raw_content_permission(kind), PermissionResponse::Deny);
    }
}

#[test]
fn warm_spare_cannot_outlive_its_profiles_last_real_view() {
    let construction = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/host/construction.rs"
    ));
    let lifecycle = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/host/lifecycle.rs"
    ));
    let ensure_spare = construction
        .split("pub(crate) fn ensure_spare(&mut self, partition: Partition)")
        .nth(1)
        .expect("native warm-spare implementation")
        .split("pub(crate) fn ensure_spare(&mut self, partition: Partition)")
        .next()
        .expect("end of native warm-spare implementation");
    assert!(ensure_spare.contains("!self.has_live_profile_view(profile)"));

    let idle_close = lifecycle
        .split("fn close_idle_spare(&mut self, profile: ProfileId)")
        .nth(1)
        .expect("idle spare retirement")
        .split("pub(crate) fn close(&mut self, id: ItemId)")
        .next()
        .expect("end of idle spare retirement");
    assert!(idle_close.contains("spare.view.close_explicit()"));
    assert!(idle_close.contains("self.web_contexts.remove(&profile)"));

    let close = lifecycle
        .split("pub(crate) fn close(&mut self, id: ItemId)")
        .nth(1)
        .expect("view close implementation")
        .split("pub(super) fn shutdown")
        .next()
        .expect("end of view close implementation");
    assert!(close.contains("self.close_idle_spare(profile)"));
}

#[test]
fn windows_raw_autofill_surfaces_are_mandatory_verified_postconditions() {
    let source = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/platform/windows/mod.rs"
    ));
    let configure = source
        .split("pub fn configure(")
        .nth(1)
        .expect("raw WebView2 configure function")
        .split("// Wry's navigation callback")
        .next()
        .expect("pre-navigation raw WebView2 policy");
    for required in [
        "settings.cast::<ICoreWebView2Settings4>()?",
        "SetIsPasswordAutosaveEnabled(false)?",
        "SetIsGeneralAutofillEnabled(false)?",
        "IsPasswordAutosaveEnabled(&mut password_autosave_enabled)?",
        "IsGeneralAutofillEnabled(&mut general_autofill_enabled)?",
        "password_autosave_enabled.as_bool() || general_autofill_enabled.as_bool()",
        "E_ACCESSDENIED",
    ] {
        assert!(
            configure.contains(required),
            "raw WebView2 autofill postcondition lost invariant: {required}"
        );
    }
}

#[test]
fn only_tabs_a_person_reads_may_take_element_fullscreen() {
    let raw_policy = raw_view_construction_policy();
    assert_eq!(raw_policy.matches("with_fullscreen_enabled(").count(), 1);
    assert!(raw_policy.contains("with_fullscreen_enabled(true)"));
    // Every other native view in the engine keeps it off explicitly; wry's
    // default follows Cargo features, not page authority.
    let sources = [
        include_str!("../construction.rs"),
        include_str!("../webext_windows.rs"),
        include_str!("../../platform/macos/agent_context.rs"),
        include_str!("../../platform/windows/agent_context.rs"),
        include_str!("../../platform/windows/agentic_semantic_probe.rs"),
        include_str!("../../platform/windows/agentic_input_probe.rs"),
    ];
    let enabled: usize = sources
        .iter()
        .map(|source| source.matches("with_fullscreen_enabled(true)").count())
        .sum();
    assert_eq!(enabled, 1);
    for source in &sources[1..] {
        assert!(source.contains("with_fullscreen_enabled(false)"));
    }
    let wry = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../vendor/wry/src/wkwebview/mod.rs"
    ));
    assert!(wry.contains("_preference.setElementFullscreenEnabled(attributes.fullscreen_enabled)"));
    let macos_preference = wry
        .split_once("_preference.setElementFullscreenEnabled(")
        .expect("public element fullscreen preference")
        .0;
    assert!(macos_preference
        .rsplit_once("#[cfg(")
        .is_some_and(|(_, cfg)| cfg.starts_with("target_os = \"macos\")")));
}

#[test]
fn the_macos_stage_never_moves_a_page_webkit_is_showing_fullscreen() {
    let stage = include_str!("../../platform/macos/stage.rs")
        .split_once("#[cfg(test)]")
        .expect("stage tests boundary")
        .0;
    let guard = "fullscreen::webkit_owns(&view.view, self)";
    for mutation in [
        "view.view.setFrame(",
        "view.view.setHidden(",
        "view.view.removeFromSuperview()",
    ] {
        let mut found = 0;
        for (at, _) in stage.match_indices(mutation) {
            found += 1;
            let before = &stage[..at];
            let function = [before.rfind("\n    fn "), before.rfind("\n    pub fn ")]
                .into_iter()
                .flatten()
                .max()
                .expect("enclosing function");
            assert!(
                before[function..].contains(guard),
                "{mutation} at byte {at} has no fullscreen guard in its function"
            );
        }
        assert!(found > 0, "{mutation} no longer appears in the stage");
    }
    // A page WebKit still holds is registered without being re-parented.
    let insert = stage
        .split_once("pub fn insert_view(")
        .expect("insert_view")
        .1
        .split_once("pub fn remove_view(")
        .expect("remove_view")
        .0;
    assert!(insert.contains("let presenting = super::fullscreen::in_transition(&view);"));
    assert!(insert.contains("if !presenting {\n            self.addSubview(&view);"));
    let pending = include_str!("../../platform/macos/mod.rs")
        .split_once("pub fn enforce_navigation_pending(")
        .expect("pending hide")
        .1
        .split_once("\n}\n")
        .expect("pending hide body")
        .0;
    assert!(pending.contains("fullscreen::in_transition(&native_webview(view))"));
}

#[test]
fn a_closed_fullscreen_page_is_handed_back_before_teardown() {
    let lifecycle = include_str!("../lifecycle.rs");
    let close = lifecycle
        .split_once("pub(crate) fn close(&mut self, id: ItemId)")
        .expect("close")
        .1
        .split_once("pub(super) fn shutdown(")
        .expect("close body")
        .0;
    let forget = close
        .find("self.forget_fullscreen(id);")
        .expect("ledger forget");
    let removed = close
        .find("let removed = self.views.remove(&id);")
        .expect("removal");
    assert!(forget < removed);
    assert!(close.contains("self.retire_fullscreen_view(view)"));
    let fullscreen = include_str!("../fullscreen.rs");
    assert!(fullscreen.contains("fullscreen::close_presentations(&view.view, finish)"));
    assert!(fullscreen.contains("schedule_presentation_timeout(RETIRE_DEADLINE"));
    // Picture in picture survives a tab switch: only closing uses the API
    // that also ends it.
    let platform = include_str!("../../platform/macos/fullscreen.rs");
    let exit = platform
        .split_once("pub(crate) fn exit(")
        .expect("exit")
        .1
        .split_once("pub(crate) fn close_presentations(")
        .expect("exit body")
        .0;
    assert!(exit.contains("WKContentWorld::defaultClientWorld"));
    assert!(!exit.contains("closeAllMediaPresentations"));
}
