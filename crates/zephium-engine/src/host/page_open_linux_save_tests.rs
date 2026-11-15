use super::*;
fn panel(id: ItemId) -> gtk::FileChooserNative {
    let slot = Rc::new(RefCell::new(None));
    until(|| {
        let result = slot.clone();
        super::super::super::try_with(move |host| {
            *result.borrow_mut() = host
                .views
                .get(&id)
                .and_then(|view| view.page_save.test_panel());
        });
        slot.borrow().is_some()
    });
    slot.take().unwrap()
}
#[test]
#[ignore = "isolated GTK; original main resource, safe publication and stale chooser"]
fn native_save_page_publishes_original_resource_without_overwrite_or_execute() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    let host = NativeHarness::new();
    let (id, page) = host.show(&format!("{}/page", fixture.origin));
    let output = tempfile::tempdir().unwrap();
    script(
        &page,
        "document.body.append('MUTATED-DOM');document.title='save-ready'",
    );
    until(|| page.title().as_deref() == Some("save-ready"));
    std::fs::write(output.path().join("page.html"), b"existing").unwrap();
    assert_eq!(host.engine.save_page(id), NativeDispatch::Scheduled);
    let chooser = panel(id);
    assert!(chooser.set_current_folder(output.path()));
    chooser.set_current_name("page.html");
    until(|| chooser.filename() == Some(output.path().join("page.html")));
    chooser.emit_by_name::<()>("response", &[&-3_i32]);
    let saved = Rc::new(RefCell::new(None));
    until(|| {
        *saved.borrow_mut() = std::fs::read_dir(output.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.path())
            .find(|p| p.is_file() && p.file_name().unwrap() != "page.html");
        saved.borrow().is_some()
    });
    let saved = saved.take().unwrap();
    let bytes = std::fs::read(&saved).unwrap();
    assert!(bytes.starts_with(b"<!doctype html>"));
    assert!(!String::from_utf8_lossy(&bytes).contains("MUTATED-DOM"));
    assert_eq!(
        std::fs::metadata(saved).unwrap().permissions().mode() & 0o111,
        0
    );
    assert_eq!(
        std::fs::read(output.path().join("page.html")).unwrap(),
        b"existing"
    );
    host.window.present();
    host.window.window().unwrap().focus(0);
    until(|| host.window.is_active());
    assert_eq!(host.engine.save_page(id), NativeDispatch::Scheduled);
    let chooser = panel(id);
    assert!(chooser.set_current_folder(output.path()));
    chooser.set_current_name("stale.html");
    page.load_uri(&format!("{}/leave", fixture.origin));
    until(|| {
        host.drain();
        page.title().as_deref() == Some("child")
    });
    chooser.emit_by_name::<()>("response", &[&-3_i32]);
    assert!(!output.path().join("stale.html").exists());
}
