use super::*;
use crate::lookup::{note_row, task_row};
use zephium_core::notes::{NoteCall, NoteDone, NoteError, NoteResponse, NoteSummary};

struct FakeNotes {
    notes: Vec<NoteSummary>,
    searches: Mutex<Vec<String>>,
}

impl zephium_core::ports::notes::Notes for FakeNotes {
    fn call(&self, _profile: ProfileId, call: NoteCall, done: NoteDone) {
        let NoteCall::List { query } = call else {
            return done(NoteResponse::Error {
                error: NoteError::Invalid,
            });
        };
        self.searches.lock().unwrap().push(query.search.clone());
        let items = self
            .notes
            .iter()
            .filter(|note| {
                note.title
                    .to_lowercase()
                    .split_whitespace()
                    .any(|word| word.starts_with(&query.search))
            })
            .cloned()
            .collect();
        done(NoteResponse::Page { items, next: None });
    }

    fn release(&self, _profile: ProfileId, done: Box<dyn FnOnce() + Send>) -> bool {
        done();
        true
    }
}

fn runs_on_any_thread<T: Send>(_: &T) {}

fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

#[test]
fn a_task_is_found_by_its_words_among_the_open_ones() {
    let store = Arc::new(FakeStore::default());
    *store.task_rows.lock().unwrap() = vec![
        task_row("done", "Renew passport", Some("2000-01-01"), true),
        task_row("other", "Book hotel", None, false),
        task_row("open", "Passport renewal", Some(&today()), false),
    ];
    let (handle, profile, _, _) = started(store.clone());

    let request = handle.find_task(profile, "renew my passport");
    runs_on_any_thread(&request);
    let found = block_on(request).expect("a task");
    assert_eq!(
        (found.id.as_str(), found.title.as_str()),
        ("open", "Passport renewal")
    );
    assert_eq!(found.due.as_deref(), Some("Today"));
    assert_eq!(*store.task_searches.lock().unwrap(), ["rene", "pass"]);

    assert_eq!(block_on(handle.find_task(profile, "buy milk")), None);
    assert_eq!(block_on(handle.find_task(profile, "the task")), None);
    assert_eq!(
        block_on(handle.find_task(ProfileId::from(9_999), "renew passport")),
        None,
        "another profile's tasks are not the person's"
    );
}

#[test]
fn a_note_is_found_by_its_title_words() {
    let notes = Arc::new(FakeNotes {
        notes: vec![
            note_row("lisbon", "Lisbon trip", 5),
            note_row("packing", "Packing list", 7),
            note_row("recipes", "Recipes", 9),
        ],
        searches: Mutex::default(),
    });
    let (handle, profile, _, _) = started(Arc::new(FakeStore::default()));
    assert!(
        handle.dispatch(Command::AttachNotes(crate::api::NotesAttachment(
            notes.clone()
        )))
    );

    let request = handle.find_note(profile, "my trip to Lisbon");
    runs_on_any_thread(&request);
    let found = block_on(request).expect("a note");
    assert_eq!(
        (found.id.as_str(), found.title.as_str()),
        ("lisbon", "Lisbon trip")
    );
    assert_eq!(*notes.searches.lock().unwrap(), ["trip", "lisb"]);

    assert_eq!(block_on(handle.find_note(profile, "train timetable")), None);
    assert_eq!(
        block_on(handle.find_note(ProfileId::from(9_999), "lisbon trip")),
        None
    );
}

#[test]
fn a_lookup_without_notes_attached_finds_nothing() {
    let (handle, profile, _, _) = started(Arc::new(FakeStore::default()));
    assert_eq!(block_on(handle.find_note(profile, "lisbon trip")), None);
}

#[test]
fn a_lookup_after_shutdown_resolves_to_nothing() {
    let store = Arc::new(FakeStore::default());
    *store.shutdown_outcome.lock().unwrap() = Some(StoreShutdownOutcome::Clean);
    let (handle, profile, space, _) = started(store);
    let _ = handle
        .shutdown()
        .recv_timeout(std::time::Duration::from_secs(5));

    assert_eq!(block_on(handle.find_task(profile, "renew passport")), None);
    assert_eq!(
        block_on(handle.open_background_tab(profile, space, None, "https://example.test/".into())),
        None
    );
}
