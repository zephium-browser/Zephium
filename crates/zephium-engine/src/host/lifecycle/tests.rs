use super::*;

#[test]
fn renderer_crash_classification_never_exposes_dead_spare() {
    let spare = ItemId::from(1);
    let live = ItemId::from(2);
    let retired = ItemId::from(3);
    assert_eq!(
        renderer_crash_target(Some(spare), false, spare),
        RendererCrashTarget::Spare
    );
    assert_eq!(
        renderer_crash_target(Some(spare), true, live),
        RendererCrashTarget::Live
    );
    assert_eq!(
        renderer_crash_target(Some(spare), false, retired),
        RendererCrashTarget::Retired
    );
}

#[cfg(not(target_os = "windows"))]
#[test]
fn shutdown_installs_completion_before_canceling_native_policy_work() {
    let source = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/host/lifecycle.rs"
    ));
    let shutdown = source
        .find("pub(super) fn shutdown(&mut self, done:")
        .expect("non-Windows shutdown barrier disappeared");
    let common = source[shutdown..]
        .find("fn shutdown_common")
        .map(|offset| shutdown + offset)
        .expect("shared shutdown teardown disappeared");
    let shutdown_source = &source[shutdown..common];
    let retain_completion = shutdown_source
        .find("self.shutdown_completion = Some(done);")
        .expect("shutdown completion is not retained");
    let teardown = shutdown_source
        .find("self.shutdown_common();")
        .expect("native teardown no longer runs");
    let settle = shutdown_source
        .find("self.finish_content_policy_shutdown_if_quiescent();")
        .expect("quiescent shutdown no longer settles immediately");
    assert!(
        retain_completion < teardown && teardown < settle,
        "a synchronous native cancellation callback must observe the retained completion"
    );
}

#[test]
fn only_a_durable_page_of_the_same_profile_reaches_its_store() {
    use zephium_core::ports::engine::Partition;
    let profile = ProfileId::generate();
    let other = ProfileId::generate();
    assert!(durable_partition_of(Partition::Default(profile), profile));
    assert!(durable_partition_of(
        Partition::Persistent(profile),
        profile
    ));
    assert!(!durable_partition_of(
        Partition::Ephemeral(profile),
        profile
    ));
    assert!(!durable_partition_of(Partition::Persistent(other), profile));
}

#[test]
fn a_temporary_page_in_use_is_kept_from_idle_and_pressure_release() {
    let profile = ProfileId::generate();
    let other = ProfileId::generate();
    assert!(temporary_page_held(Some((profile, 2)), Some(profile)));
    assert!(temporary_page_held(Some((profile, 1)), Some(profile)));
    // Its last use dropped, or another profile's spare, or none at all.
    assert!(!temporary_page_held(Some((profile, 0)), Some(profile)));
    assert!(!temporary_page_held(Some((profile, 1)), Some(other)));
    assert!(!temporary_page_held(Some((profile, 1)), None));
    assert!(!temporary_page_held(None, Some(profile)));
}
