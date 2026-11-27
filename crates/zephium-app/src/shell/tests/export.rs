use super::*;
use zephium_core::bookmarks::{BookmarkReply, BookmarkRequest};
use zephium_core::export::{ExportKind, ExportRefusal};

fn exporting(
    shell: &mut Shell,
    kind: ExportKind,
) -> std::sync::mpsc::Receiver<Result<String, ExportRefusal>> {
    let (send, receive) = std::sync::mpsc::channel();
    shell.handle(Command::Export {
        kind,
        done: crate::ExportCompletion::new(move |reply| {
            let _ = send.send(reply);
        }),
    });
    receive
}

/// Runs the real read worker over the real store, as the actor's reader thread
/// would, and hands its answer back to the shell.
fn drive(
    shell: &mut Shell,
    store: &Arc<zephium_store::SqliteStore>,
    queue: &crate::store_reads::StoreReadQueue,
) {
    let reader_store: SharedStore = store.clone();
    let reader_queue = queue.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        crate::store_reads::run_for_test(reader_store, reader_queue, tx);
    });
    let result = rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("the reader must answer an export");
    queue.stop();
    worker.join().unwrap();
    shell.handle(Command::StoreRead(result));
}

fn shell_over_real_store() -> (
    Shell,
    Arc<zephium_store::SqliteStore>,
    crate::store_reads::StoreReadQueue,
    tempfile::TempDir,
) {
    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(zephium_store::SqliteStore::open(directory.path()).unwrap());
    let mut shell = Shell::new(
        Arc::new(FakeEngine::default()),
        store.clone(),
        Arc::new(FakeChrome),
        Box::new(|_| {}),
    );
    shell.handle(Command::SetWindowSize(Size::new(1200.0, 800.0)));
    shell.handle(Command::Bootstrap);
    let queue = crate::store_reads::StoreReadQueue::new();
    shell.store_reads = Some(queue.clone());
    (shell, store, queue, directory)
}

/// The store answers within a bounded wait, which a machine running the whole
/// suite at once can outlast; seeding asks again rather than fail the test.
fn seeded(mut ask: impl FnMut() -> BookmarkReply) -> BookmarkReply {
    for _ in 0..20 {
        match ask() {
            BookmarkReply::Failed(_) => std::thread::sleep(std::time::Duration::from_millis(250)),
            reply => return reply,
        }
    }
    ask()
}

#[test]
fn bookmarks_leave_as_a_netscape_file_with_their_folders() {
    let (mut shell, store, queue, _directory) = shell_over_real_store();
    shell.persist();
    assert!(store.flush_until(std::time::Instant::now() + std::time::Duration::from_secs(30)));
    let profile = shell.windows.focused().unwrap().profile;
    let BookmarkReply::Added(folder) = seeded(|| {
        store.bookmarks(
            profile,
            BookmarkRequest::AddFolder {
                parent: None,
                title: "Reading & news".into(),
            },
        )
    }) else {
        panic!("folder");
    };
    assert!(matches!(
        seeded(|| store.bookmarks(
            profile,
            BookmarkRequest::AddLink {
                parent: Some(folder),
                title: "Rust".into(),
                url: "https://www.rust-lang.org/".into(),
                if_absent: false,
            },
        )),
        BookmarkReply::Added(_)
    ));

    let reply = exporting(&mut shell, ExportKind::Bookmarks);
    drive(&mut shell, &store, &queue);

    let html = reply.recv().unwrap().unwrap();
    assert!(html.starts_with("<!DOCTYPE NETSCAPE-Bookmark-file-1>"));
    assert!(html.contains("<DT><H3>Reading &amp; news</H3>"));
    assert!(html.contains("<DT><A HREF=\"https://www.rust-lang.org/\">Rust</A>"));
}

#[test]
fn history_leaves_as_a_csv_with_every_visit_across_pages() {
    let (mut shell, store, queue, _directory) = shell_over_real_store();
    let profile = shell.windows.focused().unwrap().profile;
    for at in 0..450 {
        store.record_visit(
            profile,
            format!("https://site{at}.example/"),
            if at == 0 {
                "=SUM(A1)".into()
            } else {
                format!("Page {at}")
            },
        );
    }
    assert!(store.flush_until(std::time::Instant::now() + std::time::Duration::from_secs(30)));

    let reply = exporting(&mut shell, ExportKind::History);
    drive(&mut shell, &store, &queue);
    let csv = reply.recv().unwrap().unwrap();
    let lines: Vec<&str> = csv.split("\r\n").filter(|line| !line.is_empty()).collect();
    assert_eq!(lines[0], "date,title,url");
    assert_eq!(lines.len(), 451, "every visit, beyond one page of 200");
    let urls: std::collections::HashSet<&str> = lines[1..]
        .iter()
        .map(|line| line.rsplit(',').next().unwrap())
        .collect();
    assert_eq!(urls.len(), 450);
    assert!(lines[1..]
        .iter()
        .any(|line| line.ends_with(",\"'=SUM(A1)\",https://site0.example/")));
}

#[test]
fn a_private_window_has_nothing_to_export_and_never_reads_the_store() {
    let (mut shell, _store, queue, _directory) = shell_over_real_store();
    shell.handle(Command::Run("window.newPrivate".into()));

    for kind in [ExportKind::Bookmarks, ExportKind::History] {
        let reply = exporting(&mut shell, kind);
        assert_eq!(reply.recv().unwrap(), Err(ExportRefusal::Private));
    }
    assert_eq!(queue.pending_surface_calls_for_test(), 0);
}

#[test]
fn only_one_export_runs_at_a_time_and_a_shutdown_answers_the_one_waiting() {
    let (mut shell, _store, queue, _directory) = shell_over_real_store();
    let first = exporting(&mut shell, ExportKind::History);
    let second = exporting(&mut shell, ExportKind::Bookmarks);
    assert_eq!(second.recv().unwrap(), Err(ExportRefusal::Unavailable));
    assert!(first.try_recv().is_err());
    assert_eq!(queue.pending_surface_calls_for_test(), 1);

    shell.fail_pending_bookmark_calls();
    assert_eq!(first.recv().unwrap(), Err(ExportRefusal::Unavailable));
}

#[test]
fn a_history_read_failure_refuses_export_instead_of_claiming_an_empty_backup() {
    let (mut shell, store, queue, _directory) = shell_over_real_store();
    shell.persist();
    assert!(store.flush_until(std::time::Instant::now() + std::time::Duration::from_secs(30)));
    // A missing profile becomes an unavailable read, never successful EOF.
    let removed = ProfileId::from(999_999);
    shell.profiles.insert(zephium_core::profiles::Profile {
        id: removed,
        name: "Unavailable".into(),
        kind: ProfileKind::Named,
    });
    shell.windows.focused_mut().unwrap().profile = removed;
    let reply = exporting(&mut shell, ExportKind::History);
    drive(&mut shell, &store, &queue);
    assert_eq!(reply.recv().unwrap(), Err(ExportRefusal::Unavailable));
}
