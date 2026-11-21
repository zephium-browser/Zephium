use super::should_seed_stage_readiness;

#[cfg(any(target_os = "macos", target_os = "windows"))]
#[test]
fn only_a_released_window_skips_its_late_layouts() {
    let mut parents = std::collections::HashMap::new();
    parents.insert(2, Some(()));
    parents.insert(3, None);
    assert!(!super::released(&parents, 1));
    assert!(!super::released(&parents, 2));
    assert!(super::released(&parents, 3));
}

#[test]
fn unchanged_layout_does_not_reseed_retained_stage_readiness() {
    assert!(should_seed_stage_readiness(true, true, true));
    assert!(!should_seed_stage_readiness(false, true, true));
    assert!(!should_seed_stage_readiness(true, false, true));
    assert!(!should_seed_stage_readiness(true, true, false));

    let source = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/host/stages.rs"));
    assert!(
        source
            .matches("should_seed_stage_readiness(\n                inserted,")
            .count()
            >= 2
    );
}
