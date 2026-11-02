use super::*;

use zephium_core::audio::AUDIO_HOLD;

fn ready() -> (Shell, Arc<FakeEngine>, Screen, ItemId) {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let item = active_id(&screen);
    navigate_and_commit(&mut shell, item, "https://music.example/");
    (shell, engine, screen, item)
}

fn audio(screen: &Screen, id: ItemId) -> Option<zephium_ipc::TabAudioView> {
    let id = id.to_string();
    last(screen)
        .tabs
        .into_iter()
        .find(|tab| tab.id == id)
        .and_then(|tab| tab.audio)
}

fn sound(shell: &mut Shell, id: ItemId, audible: bool) {
    shell.handle(Command::Engine(EngineEvent::AudioChanged { id, audible }));
}

fn mutes(engine: &FakeEngine, id: ItemId) -> Vec<String> {
    let prefix = format!("mute {id} ");
    engine
        .calls()
        .into_iter()
        .filter(|call| call.starts_with(&prefix))
        .collect()
}

#[test]
fn sound_shows_the_speaker_at_once_and_quiet_hides_it_after_the_hold() {
    let (mut shell, _engine, screen, item) = ready();
    assert_eq!(audio(&screen, item), None);
    sound(&mut shell, item, true);
    let playing = Some(zephium_ipc::TabAudioView {
        muted: false,
        can_mute: true,
    });
    assert_eq!(audio(&screen, item), playing);
    let quiet = std::time::Instant::now();
    sound(&mut shell, item, false);
    assert_eq!(audio(&screen, item), playing);
    shell.settle_audio(quiet);
    assert_eq!(audio(&screen, item), playing);
    shell.settle_audio(quiet + AUDIO_HOLD + std::time::Duration::from_millis(50));
    assert_eq!(audio(&screen, item), None);
    assert!(shell.audio_quiet.is_empty());
}

#[test]
fn a_short_sound_inside_the_hold_keeps_the_speaker_without_a_flicker() {
    let (mut shell, _engine, screen, item) = ready();
    sound(&mut shell, item, true);
    sound(&mut shell, item, false);
    let revision = |screen: &Screen| {
        last(screen)
            .tabs
            .into_iter()
            .find(|tab| tab.id == item.to_string())
            .unwrap()
            .projection_revision
    };
    let before = revision(&screen);
    sound(&mut shell, item, true);
    sound(&mut shell, item, false);
    assert_eq!(revision(&screen), before);
    assert!(audio(&screen, item).is_some());
}

#[test]
fn mute_holds_across_navigation_and_returns_with_a_new_view() {
    let (mut shell, engine, screen, item) = ready();
    let muted = shell.operation_set_tab_muted(item, true);
    assert_eq!(muted.outcome, OperationOutcome::Applied);
    assert_eq!(mutes(&engine, item), vec![format!("mute {item} true")]);
    assert_eq!(
        audio(&screen, item),
        Some(zephium_ipc::TabAudioView {
            muted: true,
            can_mute: true,
        })
    );
    assert_eq!(
        shell.operation_set_tab_muted(item, true).outcome,
        OperationOutcome::NoOp
    );

    // The native view keeps its mute across its own navigations.
    navigate_and_commit(&mut shell, item, "https://other.example/");
    assert_eq!(mutes(&engine, item).len(), 1);
    assert!(shell.items.tab(item).unwrap().audio.muted);

    // A replacement view starts unmuted, so the tab's mute is sent again.
    sound(&mut shell, item, true);
    shell.handle(Command::Engine(EngineEvent::Crashed { id: item }));
    assert!(!shell.items.tab(item).unwrap().audio.shown());
    assert!(shell.items.tab(item).unwrap().has_view());
    assert_eq!(mutes(&engine, item).len(), 2);
    assert_eq!(audio(&screen, item).map(|audio| audio.muted), Some(true));

    assert_eq!(
        shell.operation_set_tab_muted(item, false).outcome,
        OperationOutcome::Applied
    );
    assert_eq!(
        mutes(&engine, item).last().unwrap(),
        &format!("mute {item} false")
    );
    assert_eq!(audio(&screen, item), None);
}

#[test]
fn a_sleeping_tab_records_mute_and_takes_it_when_it_wakes() {
    let (mut shell, engine, _screen, item) = ready();
    let effects = shell.items.hibernate(item);
    let _ = shell.apply(effects);
    assert!(!shell.items.tab(item).unwrap().has_view());
    let calls = mutes(&engine, item).len();
    assert_eq!(
        shell.operation_set_tab_muted(item, true).outcome,
        OperationOutcome::Applied
    );
    assert_eq!(mutes(&engine, item).len(), calls);
    let effects = shell.items.ensure_view(item);
    let _ = shell.apply(effects);
    assert_eq!(
        mutes(&engine, item).last().unwrap(),
        &format!("mute {item} true")
    );
}

#[test]
fn an_engine_that_cannot_mute_only_shows_the_speaker() {
    let (mut shell, engine, screen, item) = ready();
    engine
        .cannot_mute
        .store(true, std::sync::atomic::Ordering::Release);
    assert_eq!(
        shell.operation_set_tab_muted(item, true).outcome,
        OperationOutcome::Rejected
    );
    assert!(!shell.items.tab(item).unwrap().audio.muted);
    sound(&mut shell, item, true);
    assert_eq!(
        audio(&screen, item),
        Some(zephium_ipc::TabAudioView {
            muted: false,
            can_mute: false,
        })
    );
}

#[test]
fn sound_from_a_tab_without_a_view_is_ignored() {
    let (mut shell, _engine, screen, item) = ready();
    let effects = shell.items.hibernate(item);
    let _ = shell.apply(effects);
    sound(&mut shell, item, true);
    assert_eq!(audio(&screen, item), None);
}
