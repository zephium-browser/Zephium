use super::*;

use zephium_core::media::MediaSnapshot;
use zephium_ipc::{MediaPlayerAction, MediaPlayerView};

fn ready() -> (Shell, Arc<FakeEngine>, Screen, ItemId) {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let item = active_id(&screen);
    navigate_and_commit(&mut shell, item, "https://www.music.example/");
    (shell, engine, screen, item)
}

/// A second tab in front, leaving `item` playing behind it.
fn behind(shell: &mut Shell, screen: &Screen) -> ItemId {
    shell.handle(Command::Open);
    let front = active_id(screen);
    navigate_and_commit(shell, front, "https://other.example/");
    front
}

fn media(item: &str, playing: bool) -> MediaSnapshot {
    MediaSnapshot {
        title: String::new(),
        artist: String::new(),
        duration: Some(200.0),
        position: 30.0,
        rate: 1.0,
        playing,
        waiting: false,
        ended: false,
        can_pip: true,
        in_pip: false,
        item: item.into(),
    }
}

fn report(shell: &mut Shell, id: ItemId, media: Option<MediaSnapshot>) {
    shell.handle(Command::Engine(EngineEvent::MediaChanged {
        id,
        media: media.map(Box::new),
    }));
}

fn player(shell: &Shell) -> Option<MediaPlayerView> {
    shell
        .presentation
        .last_media_player
        .borrow()
        .clone()
        .flatten()
}

fn watches(engine: &FakeEngine, id: ItemId) -> usize {
    let call = format!("media watch {id}");
    engine.calls().iter().filter(|c| **c == call).count()
}

#[test]
fn a_tab_is_watched_when_it_starts_making_sound_and_not_on_every_sound() {
    let (mut shell, engine, _screen, item) = ready();
    assert_eq!(watches(&engine, item), 0);
    shell.handle(Command::Engine(EngineEvent::AudioChanged {
        id: item,
        audible: true,
    }));
    assert_eq!(watches(&engine, item), 1);
    shell.handle(Command::Engine(EngineEvent::AudioChanged {
        id: item,
        audible: false,
    }));
    shell.handle(Command::Engine(EngineEvent::AudioChanged {
        id: item,
        audible: true,
    }));
    assert_eq!(watches(&engine, item), 1);
}

#[test]
fn the_player_shows_a_tab_playing_behind_and_hides_when_it_comes_to_front() {
    let (mut shell, _engine, screen, item) = ready();
    report(&mut shell, item, Some(media("1", true)));
    assert_eq!(player(&shell), None, "the playing tab is in front");
    let front = behind(&mut shell, &screen);
    let shown = player(&shell).expect("a player for the tab behind");
    assert_eq!(shown.tab_id, item.to_string());
    assert_eq!(shown.artist, "music.example");
    assert!(shown.playing && shown.can_pip && shown.can_mute && !shown.muted);
    assert_eq!(shown.duration, Some(200.0));
    shell.handle(Command::Activate(item));
    assert_eq!(player(&shell), None);
    shell.handle(Command::Activate(front));
    assert!(player(&shell).is_some());
}

#[test]
fn the_player_follows_its_page_and_goes_when_the_media_or_the_tab_does() {
    let (mut shell, _engine, screen, item) = ready();
    report(&mut shell, item, Some(media("1", true)));
    behind(&mut shell, &screen);
    report(&mut shell, item, Some(media("1", false)));
    assert!(!player(&shell).unwrap().playing, "paused keeps the player");
    let mut ended = media("1", false);
    ended.ended = true;
    report(&mut shell, item, Some(ended));
    assert_eq!(player(&shell), None);
    report(&mut shell, item, Some(media("1", true)));
    assert!(player(&shell).is_some());
    report(&mut shell, item, None);
    assert_eq!(player(&shell), None);
    report(&mut shell, item, Some(media("1", true)));
    shell.handle(Command::Close(item));
    assert_eq!(player(&shell), None);
}

#[test]
fn closing_the_player_holds_until_the_tab_plays_something_else() {
    let (mut shell, _engine, screen, item) = ready();
    report(&mut shell, item, Some(media("1", true)));
    behind(&mut shell, &screen);
    let closed = shell.operation_media_player(item, MediaPlayerAction::Close);
    assert_eq!(closed.outcome, OperationOutcome::Applied);
    assert_eq!(player(&shell), None);
    report(&mut shell, item, Some(media("1", false)));
    report(&mut shell, item, Some(media("1", true)));
    assert_eq!(player(&shell), None);
    report(&mut shell, item, Some(media("2", true)));
    assert!(player(&shell).is_some());
}

#[test]
fn the_player_acts_only_on_the_tab_it_shows() {
    let (mut shell, engine, screen, item) = ready();
    report(&mut shell, item, Some(media("1", true)));
    let front = behind(&mut shell, &screen);
    let pause = shell.operation_media_player(front, MediaPlayerAction::Pause);
    assert_eq!(pause.outcome, OperationOutcome::Rejected);
    for (action, call) in [
        (MediaPlayerAction::Pause, "Pause"),
        (MediaPlayerAction::Play, "Play"),
        (MediaPlayerAction::Seek { seconds: 12.0 }, "Seek(12.0)"),
        (MediaPlayerAction::PictureInPicture, "PictureInPicture"),
    ] {
        let done = shell.operation_media_player(item, action);
        assert_eq!(done.outcome, OperationOutcome::Applied, "{call}");
        assert!(
            engine.calls().contains(&format!("media {item} {call}")),
            "{call}"
        );
    }
    let wild = shell.operation_media_player(item, MediaPlayerAction::Seek { seconds: f64::NAN });
    assert_eq!(wild.outcome, OperationOutcome::Rejected);
    assert!(!engine.calls().iter().any(|call| call.contains("NaN")));
}

#[test]
fn the_player_mutes_its_tab_and_names_what_plays() {
    let (mut shell, engine, screen, item) = ready();
    let mut song = media("1", true);
    song.title = "Song".into();
    song.artist = "Band".into();
    report(&mut shell, item, Some(song));
    behind(&mut shell, &screen);
    let shown = player(&shell).unwrap();
    assert_eq!(
        (shown.title.as_str(), shown.artist.as_str()),
        ("Song", "Band")
    );
    let muted = shell.operation_set_tab_muted(item, true);
    assert_eq!(muted.outcome, OperationOutcome::Applied);
    assert!(engine.calls().contains(&format!("mute {item} true")));
    assert!(player(&shell).unwrap().muted);
}

#[test]
fn a_page_that_went_away_takes_its_player_with_it() {
    let (mut shell, _engine, screen, item) = ready();
    report(&mut shell, item, Some(media("1", true)));
    behind(&mut shell, &screen);
    assert!(player(&shell).is_some());
    shell.handle(Command::Engine(EngineEvent::Crashed { id: item }));
    assert_eq!(player(&shell), None);
    report(&mut shell, item, Some(media("1", true)));
    assert_eq!(player(&shell), None, "a report from a view that is gone");
}

#[test]
fn the_player_follows_its_tab_across_spaces_and_goes_to_it() {
    let (mut shell, _engine, _screen, item) = ready();
    let home = shell.windows.focused().unwrap().space;
    report(&mut shell, item, Some(media("1", true)));
    shell.handle(Command::Space(Box::new(crate::SpaceCommand::Create {
        name: "Work".into(),
        look: Default::default(),
    })));
    assert_ne!(shell.windows.focused().unwrap().space, home);
    assert_eq!(player(&shell).unwrap().tab_id, item.to_string());
    let muted = shell.operation_set_tab_muted(item, true);
    assert_eq!(
        muted.outcome,
        OperationOutcome::Applied,
        "the player mutes it from here"
    );
    let shown = shell.operation_media_player(item, MediaPlayerAction::Show);
    assert_ne!(shown.outcome, OperationOutcome::Rejected);
    let window = shell.windows.focused().unwrap();
    assert_eq!((window.space, window.active), (home, Some(item)));
    assert_eq!(player(&shell), None);
}
