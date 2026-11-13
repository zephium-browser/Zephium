use super::*;

#[test]
fn a_hung_capture_allows_healthy_tabs_to_progress_but_keeps_native_work_bounded() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let first = active_id(&screen);
    navigate_and_commit(&mut shell, first, "first.example");
    shell.handle(Command::Open);
    let second = active_id(&screen);
    navigate_and_commit(&mut shell, second, "second.example");
    let window = shell.windows.focused().unwrap().id;
    shell.self_queue = Some(crate::actor::CommandQueue::new());
    shell.queue_tab_preview(window, first, false);
    shell.capture_tab_preview();
    shell.queue_tab_preview(window, second, false);
    assert_eq!(engine.tab_preview_completions.lock().unwrap().len(), 1);
    shell.expire_tab_preview(std::time::Instant::now() + std::time::Duration::from_secs(3));
    assert_eq!(engine.tab_preview_completions.lock().unwrap().len(), 2);
    // These callbacks intentionally never arrive. Retry and close/retire
    // attempts must not pretend those native operations were cancelled.
    for n in 0..30 {
        shell.handle(Command::Open);
        let tab = active_id(&screen);
        navigate_and_commit(&mut shell, tab, &format!("tab{n}.example"));
        shell.queue_tab_preview(window, tab, false);
        shell.capture_tab_preview();
        shell.expire_tab_preview(std::time::Instant::now() + std::time::Duration::from_secs(3));
        shell.record_view_retirement(tab);
    }
    assert_eq!(engine.tab_preview_completions.lock().unwrap().len(), 16);
    for _ in 0..100 {
        shell.queue_tab_preview(window, first, false);
        shell.capture_tab_preview();
        shell.expire_tab_preview(std::time::Instant::now() + std::time::Duration::from_secs(3));
    }
    assert_eq!(engine.tab_preview_completions.lock().unwrap().len(), 16);
}

#[test]
fn an_earlier_shared_maintenance_wake_rearms_the_pending_capture_deadline() {
    let (mut shell, _, screen) = setup();
    shell.handle(Command::Bootstrap);
    let tab = active_id(&screen);
    navigate_and_commit(&mut shell, tab, "preview.example");
    let window = shell.windows.focused().unwrap().id;
    let queue = crate::actor::CommandQueue::new();
    shell.self_queue = Some(queue.clone());
    let early = std::time::Instant::now();
    queue.schedule_dormancy(early);
    shell.queue_tab_preview(window, tab, false);
    shell.capture_tab_preview();
    let _ = queue.wait_for_timer(early + std::time::Duration::from_secs(60));
    assert!(queue.dormancy_deadline().is_none());
    shell.expire_tab_preview(std::time::Instant::now());
    let rearmed = queue
        .dormancy_deadline()
        .expect("the native capture still owns a deadline");
    assert!(rearmed > early && rearmed <= early + std::time::Duration::from_secs(3));
}
