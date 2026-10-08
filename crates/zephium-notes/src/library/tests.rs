use std::fs;

use zephium_core::notes::{NoteQuery, NoteSummary};

use super::*;

const NOW: i64 = 1_800_000_000_000;

fn library() -> (tempfile::TempDir, Library) {
    let base = tempfile::tempdir().unwrap();
    let folder = Folder::open(base.path(), &["notes", "profile", "Notes"]).unwrap();
    (base, Library::in_folder(folder))
}

fn request(n: u32) -> String {
    format!("request-{n:012}")
}

fn applied((response, _): (NoteResponse, Changes)) -> NoteSummary {
    match response {
        NoteResponse::Applied { summary, .. } => summary,
        other => panic!("expected applied, got {other:?}"),
    }
}

fn create(library: &mut Library, n: u32, markdown: &str) -> NoteSummary {
    applied(library.call(
        NoteCall::Create {
            request_id: request(n),
            markdown: markdown.into(),
        },
        NOW,
    ))
}

fn write(
    library: &mut Library,
    n: u32,
    note: &NoteSummary,
    markdown: &str,
) -> (NoteResponse, Changes) {
    save(library, n, note, markdown, false)
}

/// A write made as the person leaves the note, or stops retitling it.
fn settle(
    library: &mut Library,
    n: u32,
    note: &NoteSummary,
    markdown: &str,
) -> (NoteResponse, Changes) {
    save(library, n, note, markdown, true)
}

fn save(
    library: &mut Library,
    n: u32,
    note: &NoteSummary,
    markdown: &str,
    settle: bool,
) -> (NoteResponse, Changes) {
    library.call(
        NoteCall::Write {
            request_id: request(n),
            id: note.id.clone(),
            base_revision: note.revision.clone(),
            markdown: markdown.into(),
            settle,
        },
        NOW,
    )
}

fn list(library: &mut Library, search: &str, trashed: bool) -> Vec<NoteSummary> {
    match library
        .call(
            NoteCall::List {
                query: NoteQuery {
                    search: search.into(),
                    trashed,
                    after: None,
                    limit: 50,
                },
            },
            NOW,
        )
        .0
    {
        NoteResponse::Page { items, .. } => items,
        other => panic!("expected a page, got {other:?}"),
    }
}

fn markdown(library: &mut Library, id: &str) -> String {
    match library.call(NoteCall::Get { id: id.into() }, NOW).0 {
        NoteResponse::Record { record } => record.markdown,
        other => panic!("expected a record, got {other:?}"),
    }
}

fn on_disk(library: &Library, path: &str) -> String {
    fs::read_to_string(library.root().join(path)).unwrap()
}

#[test]
fn a_new_note_is_a_file_named_after_its_title() {
    let (_base, mut library) = library();
    let note = create(&mut library, 1, "# Trip: Lisbon\n\nBook the train.");
    assert_eq!(note.path, "Trip Lisbon.md");
    assert_eq!(note.title, "Trip: Lisbon");
    assert_eq!(note.preview, "Book the train.");
    assert_eq!(
        on_disk(&library, "Trip Lisbon.md"),
        "# Trip: Lisbon\n\nBook the train."
    );
    let untitled = create(&mut library, 2, "just text");
    assert_eq!(
        (untitled.path.as_str(), untitled.title.as_str()),
        ("Untitled.md", "Untitled")
    );
    let replay = create(&mut library, 1, "# Trip: Lisbon\n\nBook the train.");
    assert_eq!(replay.id, note.id);
    assert_eq!(list(&mut library, "", false).len(), 2);
}

#[test]
fn writes_prove_the_version_they_edited() {
    let (_base, mut library) = library();
    let note = create(&mut library, 1, "# Plans\n\nOne");
    let saved = applied(write(&mut library, 2, &note, "# Plans\n\nTwo"));
    assert_eq!(on_disk(&library, "Plans.md"), "# Plans\n\nTwo");
    // The first attempt's reply was lost; the retry changes nothing more.
    assert_eq!(
        applied(write(&mut library, 2, &note, "# Plans\n\nTwo")).revision,
        saved.revision
    );
    // Another app edited the file; a stale write is refused, not merged.
    fs::write(library.root().join("Plans.md"), "# Plans\n\nFrom elsewhere").unwrap();
    match write(&mut library, 3, &saved, "# Plans\n\nThree").0 {
        NoteResponse::Conflict { current } => {
            assert_eq!(current.markdown, "# Plans\n\nFrom elsewhere")
        }
        other => panic!("expected a conflict, got {other:?}"),
    }
    assert_eq!(on_disk(&library, "Plans.md"), "# Plans\n\nFrom elsewhere");
}

#[test]
fn only_a_new_title_is_news_beyond_the_note_itself() {
    let (_base, mut library) = library();
    let note = create(&mut library, 1, "# Plans\n\nOne");
    let (response, changes) = write(&mut library, 2, &note, "# Plans\n\nTwo");
    assert_eq!(changes.ids, std::slice::from_ref(&note.id));
    assert!(!changes.reset);
    assert!(changes.links.is_empty());
    let saved = applied((response, changes));
    // A retitle changes where links lead, not which notes there are.
    let (_, changes) = write(&mut library, 3, &saved, "# Road  Map\n\nTwo");
    assert!(!changes.reset);
    assert_eq!(changes.links, ["plans", "road map"]);
}

#[test]
fn typing_a_new_title_renames_the_file_once_it_settles() {
    let (_base, mut library) = library();
    let note = create(&mut library, 1, "# G");
    assert_eq!(note.path, "G.md");
    let note = applied(write(&mut library, 2, &note, "# Gr"));
    let (response, changes) = write(&mut library, 3, &note, "# Groceries");
    assert!(!changes.reset);
    let note = applied((response, changes));
    assert_eq!(
        (note.path.as_str(), note.title.as_str()),
        ("G.md", "Groceries")
    );
    // Leaving the note writes nothing new and gives the file its name.
    let (response, changes) = settle(&mut library, 4, &note, "# Groceries");
    assert_eq!(changes.ids, std::slice::from_ref(&note.id));
    assert_eq!(changes.links, ["g", "groceries"]);
    assert!(!changes.reset);
    let settled = applied((response, changes));
    assert_eq!(settled.path, "Groceries.md");
    assert_eq!(settled.revision, note.revision);
    assert!(library.root().join("Groceries.md").exists());
    assert!(!library.root().join("G.md").exists());
    // Settling again has nothing left to do.
    let (_, changes) = settle(&mut library, 5, &settled, "# Groceries");
    assert!(changes.ids.is_empty() && changes.links.is_empty());
}

#[test]
fn a_retitle_not_yet_settled_survives_a_restart() {
    let base = tempfile::tempdir().unwrap();
    let mut library = Library::open(base.path(), "profile").unwrap();
    let note = create(&mut library, 1, "# Draft");
    let note = applied(write(&mut library, 2, &note, "# Final"));
    drop(library);
    let mut library = Library::open(base.path(), "profile").unwrap();
    let note = applied(settle(&mut library, 3, &note, "# Final\n\nDone"));
    assert_eq!(note.path, "Final.md");
}

#[test]
fn a_file_renamed_elsewhere_mid_retitle_keeps_its_name() {
    let (_base, mut library) = library();
    let note = create(&mut library, 1, "# Ideas");
    applied(write(&mut library, 2, &note, "# Better ideas"));
    fs::rename(
        library.root().join("Ideas.md"),
        library.root().join("Keep me.md"),
    )
    .unwrap();
    library.reconcile(NOW).unwrap();
    let note = list(&mut library, "", false).remove(0);
    let note = applied(settle(&mut library, 3, &note, "# Best ideas"));
    assert_eq!(note.path, "Keep me.md");
}

#[test]
fn the_file_name_follows_the_title_until_someone_renames_it() {
    let (_base, mut library) = library();
    create(&mut library, 1, "# Ideas");
    let note = create(&mut library, 2, "# Ideas");
    assert_eq!(note.path, "Ideas 2.md");
    let note = applied(settle(&mut library, 3, &note, "# Groceries\n\nMilk"));
    assert_eq!(note.path, "Groceries.md");
    let note = applied(settle(&mut library, 4, &note, "# groceries\n\nMilk"));
    assert_eq!(note.path, "groceries.md");
    fs::rename(
        library.root().join("groceries.md"),
        library.root().join("Shopping list.md"),
    )
    .unwrap();
    library.reconcile(NOW).unwrap();
    let note = list(&mut library, "", false)
        .into_iter()
        .find(|n| n.id == note.id)
        .unwrap();
    assert_eq!(note.path, "Shopping list.md");
    let note = applied(settle(&mut library, 5, &note, "# Weekly shop\n\nMilk"));
    assert_eq!(note.path, "Shopping list.md");
    assert_eq!(note.title, "Weekly shop");
}

#[test]
fn trash_restore_and_delete_move_the_file() {
    let (_base, mut library) = library();
    let note = create(&mut library, 1, "# Old");
    applied(library.call(
        NoteCall::Trash {
            id: note.id.clone(),
        },
        NOW,
    ));
    assert!(library.root().join(".trash/Old.md").exists());
    assert!(list(&mut library, "", false).is_empty());
    assert_eq!(list(&mut library, "", true)[0].path, "Old.md");
    assert!(matches!(
        write(&mut library, 2, &note, "# Old\n\nedit").0,
        NoteResponse::Error {
            error: NoteError::ReadOnly
        }
    ));
    create(&mut library, 3, "# Old");
    let restored = applied(library.call(
        NoteCall::Restore {
            id: note.id.clone(),
        },
        NOW,
    ));
    assert_eq!(restored.path, "Old 2.md");
    applied(library.call(
        NoteCall::Trash {
            id: note.id.clone(),
        },
        NOW,
    ));
    assert!(matches!(
        library
            .call(
                NoteCall::Delete {
                    id: note.id.clone()
                },
                NOW
            )
            .0,
        NoteResponse::Done
    ));
    assert!(!library.root().join(".trash/Old 2.md").exists());
    assert!(list(&mut library, "", true).is_empty());
}

#[test]
fn expired_trash_is_deleted_for_good() {
    let (_base, mut library) = library();
    let note = create(&mut library, 1, "# Old");
    applied(library.call(
        NoteCall::Trash {
            id: note.id.clone(),
        },
        NOW,
    ));
    assert!(library
        .expire_trash(NOW + TRASH_RETENTION_MS)
        .unwrap()
        .is_empty());
    let changes = library.expire_trash(NOW + TRASH_RETENTION_MS + 1).unwrap();
    assert_eq!(changes.ids, [note.id]);
    assert!(!library.root().join(".trash/Old.md").exists());
}

#[test]
fn outside_changes_are_picked_up_and_moved_files_keep_their_identity() {
    let (_base, mut library) = library();
    let kept = create(&mut library, 1, "# Kept");
    applied(library.call(
        NoteCall::SetPinned {
            id: kept.id.clone(),
            pinned: true,
        },
        NOW,
    ));
    let gone = create(&mut library, 2, "# Gone");
    fs::write(library.root().join("Added.md"), "# Added\n\nFrom Finder").unwrap();
    fs::create_dir(library.root().join("Archive")).unwrap();
    fs::rename(
        library.root().join("Kept.md"),
        library.root().join("Archive/Kept.md"),
    )
    .unwrap();
    fs::remove_file(library.root().join("Gone.md")).unwrap();
    let changes = library.reconcile(NOW).unwrap();
    assert!(changes.reset);
    assert!(changes.ids.contains(&gone.id));
    let notes = list(&mut library, "", false);
    assert_eq!(notes.len(), 2);
    let moved = notes.iter().find(|n| n.id == kept.id).unwrap();
    assert_eq!(moved.path, "Archive/Kept.md");
    assert!(moved.pinned);
    assert_eq!(list(&mut library, "finder", false)[0].title, "Added");
    assert!(library.reconcile(NOW).unwrap().is_empty());
}

#[test]
fn line_endings_and_byte_order_marks_survive_an_edit() {
    let (_base, mut library) = library();
    fs::write(
        library.root().join("Windows.md"),
        "\u{feff}# Windows\r\n\r\nOne\r\n",
    )
    .unwrap();
    library.reconcile(NOW).unwrap();
    let note = list(&mut library, "", false).remove(0);
    assert_eq!(markdown(&mut library, &note.id), "# Windows\n\nOne\n");
    applied(write(&mut library, 1, &note, "# Windows\n\nOne\nTwo\n"));
    assert_eq!(
        fs::read(library.root().join("Windows.md")).unwrap(),
        "\u{feff}# Windows\r\n\r\nOne\r\nTwo\r\n".as_bytes()
    );
}

#[test]
fn notes_that_cannot_be_edited_safely_are_read_only() {
    let (_base, mut library) = library();
    fs::write(
        library.root().join("Big.md"),
        vec![b'a'; zephium_core::notes::MAX_NOTE_BYTES + 1],
    )
    .unwrap();
    fs::write(library.root().join("Latin1.md"), b"caf\xe9").unwrap();
    library.reconcile(NOW).unwrap();
    let notes = list(&mut library, "", false);
    assert!(notes.iter().all(|note| !note.editable));
    let big = notes.iter().find(|n| n.path == "Big.md").unwrap();
    assert!(matches!(
        library.call(NoteCall::Get { id: big.id.clone() }, NOW).0,
        NoteResponse::Error {
            error: NoteError::TooLarge
        }
    ));
    let latin = notes
        .iter()
        .find(|n| n.path == "Latin1.md")
        .unwrap()
        .clone();
    assert!(matches!(
        write(&mut library, 1, &latin, "cafe").0,
        NoteResponse::Error {
            error: NoteError::ReadOnly
        }
    ));
    assert_eq!(
        fs::read(library.root().join("Latin1.md")).unwrap(),
        b"caf\xe9"
    );
}

#[test]
fn wiki_links_resolve_and_report_backlinks() {
    let (_base, mut library) = library();
    let plans = create(&mut library, 1, "# Summer plans");
    let ideas = create(
        &mut library,
        2,
        "# Ideas\n\nSee [[Summer plans]] and [[Nowhere]].",
    );
    match library
        .call(
            NoteCall::Resolve {
                targets: vec!["summer PLANS".into(), "Nowhere".into()],
            },
            NOW,
        )
        .0
    {
        NoteResponse::Targets { items } => {
            assert_eq!(
                items[0].note.as_ref().map(|n| n.id.as_str()),
                Some(plans.id.as_str())
            );
            assert!(items[1].note.is_none());
        }
        other => panic!("expected targets, got {other:?}"),
    }
    match library
        .call(
            NoteCall::Backlinks {
                id: plans.id.clone(),
            },
            NOW,
        )
        .0
    {
        NoteResponse::Page { items, .. } => assert_eq!(items[0].id, ideas.id),
        other => panic!("expected a page, got {other:?}"),
    }
}
