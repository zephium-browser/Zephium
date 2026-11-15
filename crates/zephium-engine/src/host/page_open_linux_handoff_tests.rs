use super::*;

#[test]
#[ignore = "isolated GTK; launcher ownership handoff and bounded refusal"]
fn native_beforeunload_waits_for_browser_foreground_and_times_out_safely() {
    let fixture = Fixture::new();
    let host = NativeHarness::new();
    let (id, page) = host.show(&format!("{}/page", fixture.origin));
    script(&page, "document.querySelector('#script').onclick=()=>{onbeforeunload=e=>{e.preventDefault();e.returnValue='fixture'};document.title='armed'};document.title='handoff-ready'");
    until(|| page.title().as_deref() == Some("handoff-ready"));
    click(&page, 140, 1, gtk::gdk::ModifierType::empty());
    until(|| page.title().as_deref() == Some("armed"));
    for (index, response) in [
        None,
        Some(gtk::ResponseType::Cancel),
        Some(gtk::ResponseType::Ok),
    ]
    .into_iter()
    .enumerate()
    {
        let launcher = gtk::Window::new(gtk::WindowType::Toplevel);
        launcher.show_all();
        launcher.present();
        launcher.window().unwrap().focus(0);
        until(|| launcher.is_active() && !host.window.is_active());
        let request = zephium_core::ports::engine::NavigationRequestId(80 + index as u64);
        assert!(host
            .engine
            .navigate(id, &format!("{}/leave", fixture.origin), request));
        // Let the native beforeunload callback arrive while the launcher owns focus.
        let wait = Instant::now();
        until(|| {
            host.drain();
            wait.elapsed() > Duration::from_millis(200)
        });
        assert!(!gtk::Window::list_toplevels()
            .iter()
            .any(|w| w.is_visible() && w.is::<gtk::Dialog>()));
        if let Some(response) = response {
            launcher.close();
            host.window.present();
            host.window.window().unwrap().focus(0);
            visible_dialog().response(response);
        }
        if response == Some(gtk::ResponseType::Ok) {
            until(|| {
                host.drain();
                !page.is_loading() && page.title().as_deref() == Some("child")
            });
        } else {
            until(|| {
                host.drain().iter().any(|event| matches!(event, EngineEvent::NavigationFailed { request: failed, .. } if *failed == request))
            });
            launcher.close();
            script(&page, "document.title=location.pathname");
            until(|| page.title().as_deref() == Some("/page"));
        }
    }
}
