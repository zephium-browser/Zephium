use super::*;
use crate::actor::Handle;
use std::sync::mpsc::sync_channel;
use zephium_core::ids::WindowId;

use zephium_core::ports::engine::{
    ContentScope, NativeAction, UserContentApplyFailure, UserContentGeneration,
    UserContentSettlement, ZoomRequestId,
};
use zephium_core::split::Pane;

fn test_shutdown_deadline() -> std::time::Instant {
    std::time::Instant::now() + crate::shell::END_TO_END_SHUTDOWN_TIMEOUT
}

#[test]
fn sidebar_guide_cleanup_is_reserved_and_not_replaced_by_preview() {
    assert!(command_is_critical(&Command::SidebarResizeGuide(None)));
    let mut queue = VecDeque::from([Command::SidebarResizeGuide(None)]);
    assert!(enqueue(
        &mut queue,
        Command::SidebarResizeGuide(Some(310.0)),
        2,
        false
    )
    .is_ok());
    assert_eq!(queue.len(), 2);
    assert!(matches!(
        queue.front(),
        Some(Command::SidebarResizeGuide(None))
    ));
}

#[test]
fn foreground_capacity_deadlines_are_bounded_and_wake_before_maintenance() {
    let queue = CommandQueue::new();
    let future = std::time::Instant::now() + std::time::Duration::from_secs(30);
    for n in 0..(crate::shell::MAX_VISIBLE_PANES + 4) {
        queue.schedule_view_capacity(ItemId::from(n as u128 + 1), future);
    }
    assert_eq!(
        queue
            .inner
            .timer_state
            .lock()
            .unwrap()
            .capacity_deadlines
            .len(),
        crate::shell::MAX_VISIBLE_PANES
    );
    let id = ItemId::from(1);
    queue.schedule_view_capacity(id, std::time::Instant::now());
    assert!(
        matches!(queue.wait_for_timer(future), TimerWake::ViewCapacity { id: ready } if ready == id)
    );
    queue.cancel_view_capacity(ItemId::from(2));
    assert_eq!(
        queue
            .inner
            .timer_state
            .lock()
            .unwrap()
            .capacity_deadlines
            .len(),
        crate::shell::MAX_VISIBLE_PANES - 2
    );
}

#[test]
fn normal_memory_pressure_replaces_warning_across_fifo_at_capacity() {
    use zephium_core::ports::engine::MemoryPressure;
    let mut queue = VecDeque::from([
        Command::SetMemoryPressure(MemoryPressure::Critical),
        Command::Open,
    ]);
    assert!(command_is_critical(&Command::SetMemoryPressure(
        MemoryPressure::Normal
    )));
    assert!(enqueue(
        &mut queue,
        Command::SetMemoryPressure(MemoryPressure::Normal),
        2,
        true
    )
    .is_ok());
    assert_eq!(queue.len(), 2);
    assert!(matches!(queue.front(), Some(Command::Open)));
    assert!(matches!(
        queue.back(),
        Some(Command::SetMemoryPressure(MemoryPressure::Normal))
    ));
}

#[test]
fn user_content_settlements_coalesce_by_scope_not_generation() {
    let scope = ContentScope::Profile(ProfileId::from(60));
    let event = |generation| EngineEvent::UserContentSettled {
        scope,
        requested: UserContentGeneration::new(generation).unwrap(),
        settlement: UserContentSettlement::Unavailable {
            failure: UserContentApplyFailure::UnsupportedPlatform,
        },
    };
    assert_eq!(CoalescedKey::of(&event(1)), CoalescedKey::of(&event(2)));
    assert_ne!(
        CoalescedKey::of(&event(1)),
        CoalescedKey::of(&EngineEvent::UserContentSettled {
            scope: ContentScope::Profile(ProfileId::from(61)),
            requested: UserContentGeneration::new(1).unwrap(),
            settlement: UserContentSettlement::Unavailable {
                failure: UserContentApplyFailure::UnsupportedPlatform,
            },
        })
    );
}

#[test]
fn action_invalidations_coalesce_per_profile() {
    let first = ProfileId::from(62);
    let second = ProfileId::from(63);
    let event = |profile| EngineEvent::ExtensionActionsInvalidated { profile };
    assert_eq!(
        CoalescedKey::of(&event(first)),
        CoalescedKey::of(&event(first))
    );
    assert_ne!(
        CoalescedKey::of(&event(first)),
        CoalescedKey::of(&event(second))
    );
}

#[test]
fn blocker_preference_retry_timer_is_exact_and_profile_bounded() {
    let queue = CommandQueue::new();
    let profile = ProfileId::from(61);
    let now = std::time::Instant::now();
    queue.schedule_blocker_preference_reconciliation(profile, 4, now);
    queue.schedule_blocker_preference_reconciliation(profile, 5, now);
    assert!(matches!(
        queue.wait_for_timer(now + std::time::Duration::from_secs(1)),
        TimerWake::BlockerPreference {
            profile: found,
            token: 5,
        } if found == profile
    ));
}

#[test]
fn page_permission_timer_is_single_flight_exact_and_cancellable() {
    let queue = CommandQueue::new();
    let profile = ProfileId::from(71);
    let item = ItemId::from(72);
    let request = zephium_core::permissions::PagePermissionRequestId::new(73).unwrap();
    let now = std::time::Instant::now();

    queue.schedule_page_permission(profile, item, request, now);
    assert!(matches!(
        queue.wait_for_timer(now + std::time::Duration::from_secs(1)),
        TimerWake::PagePermission {
            profile: found_profile,
            item: found_item,
            request: found_request,
        } if found_profile == profile && found_item == item && found_request == request
    ));

    queue.schedule_page_permission(profile, item, request, now);
    queue.cancel_page_permission(profile, item, request);
    assert!(queue
        .inner
        .timer_state
        .lock()
        .unwrap()
        .page_permission_deadline
        .is_none());
}

#[test]
fn blocker_catalog_poll_timer_is_single_flight_and_operation_exact() {
    let queue = CommandQueue::new();
    let now = std::time::Instant::now();
    queue.schedule_blocker_catalog_poll(0, 7, now);
    queue.schedule_blocker_catalog_poll(4, 0, now + std::time::Duration::from_secs(1));
    queue.schedule_blocker_catalog_poll(3, 9, now);
    assert!(queue
        .inner
        .timer_state
        .lock()
        .unwrap()
        .blocker_catalog_deadline
        .is_some_and(|(_, operation, attempt)| operation == 4 && attempt == 0));

    // The reserved internal activation wake is deliberately lower authority
    // than an exact accepted user refresh and cannot displace its poll.
    queue.schedule_blocker_catalog_poll(0, 8, now);
    assert!(queue
        .inner
        .timer_state
        .lock()
        .unwrap()
        .blocker_catalog_deadline
        .is_some_and(|(_, operation, attempt)| operation == 4 && attempt == 0));

    queue.schedule_blocker_catalog_poll(5, 1, now);
    assert!(matches!(
        queue.wait_for_timer(now + std::time::Duration::from_secs(1)),
        TimerWake::BlockerCatalog {
            operation: 5,
            attempt: 1,
        }
    ));

    queue.schedule_blocker_catalog_poll(6, 2, now);
    queue.cancel_blocker_catalog_poll(5);
    assert!(queue
        .inner
        .timer_state
        .lock()
        .unwrap()
        .blocker_catalog_deadline
        .is_some());
    queue.cancel_blocker_catalog_poll(6);
    assert!(queue
        .inner
        .timer_state
        .lock()
        .unwrap()
        .blocker_catalog_deadline
        .is_none());
}

#[test]
fn presentation_timer_is_bounded_per_item_and_duplicate_token_keeps_earliest_deadline() {
    let queue = CommandQueue::new();
    let id = ItemId::from(7);
    let first = NavigationPresentationId::from_raw(1);
    let second = NavigationPresentationId::from_raw(2);
    let now = std::time::Instant::now();
    let first_wake = now + std::time::Duration::from_secs(1);
    let first_hard = now + std::time::Duration::from_secs(5);
    queue.schedule_presentation(id, first, first_wake, first_hard);
    queue.schedule_presentation(
        id,
        first,
        now + std::time::Duration::from_secs(2),
        now + std::time::Duration::from_secs(6),
    );
    {
        let timer = queue.inner.timer_state.lock().unwrap();
        assert_eq!(timer.presentation_deadlines.len(), 1);
        assert_eq!(
            timer.presentation_deadlines[&id],
            PresentationDeadline {
                wake: first_wake,
                hard: first_hard,
                navigation: first,
            }
        );
    }

    queue.schedule_presentation(
        id,
        second,
        now + std::time::Duration::from_secs(3),
        now + std::time::Duration::from_secs(7),
    );
    assert_eq!(
        queue
            .inner
            .timer_state
            .lock()
            .unwrap()
            .presentation_deadlines[&id],
        PresentationDeadline {
            wake: first_wake,
            hard: first_hard,
            navigation: second,
        }
    );
    let shortened_hard = now + std::time::Duration::from_secs(4);
    queue.schedule_presentation(id, second, now, shortened_hard);
    assert!(matches!(
        queue.wait_for_timer(now + std::time::Duration::from_secs(1)),
        TimerWake::Presentation {
            id: observed,
            navigation,
            hard_deadline,
        } if observed == id && navigation == second && hard_deadline == shortened_hard
    ));
    assert!(queue
        .inner
        .timer_state
        .lock()
        .unwrap()
        .presentation_deadlines
        .is_empty());

    // A wake for the retired navigation may escape the timer lock before
    // the replacement is scheduled. Queue backpressure must not let that
    // old wake overwrite the replacement obligation while re-arming.
    let replacement_wake = now + std::time::Duration::from_secs(4);
    let replacement_hard = now + std::time::Duration::from_secs(8);
    queue.schedule_presentation(id, second, replacement_wake, replacement_hard);
    queue.retry_presentation(
        id,
        first,
        now + std::time::Duration::from_millis(25),
        now + std::time::Duration::from_secs(9),
    );
    assert_eq!(
        queue
            .inner
            .timer_state
            .lock()
            .unwrap()
            .presentation_deadlines[&id],
        PresentationDeadline {
            wake: replacement_wake,
            hard: replacement_hard,
            navigation: second,
        }
    );
}

#[test]
fn completed_presentation_coalesces_after_its_committed_url_not_before_it() {
    let id = ItemId::from(7);
    let navigation = NavigationPresentationId::from_raw(9);
    let mut commands = VecDeque::new();
    enqueue(
        &mut commands,
        Command::Engine(EngineEvent::UrlChanged {
            id,
            url: "https://final.example/".into(),
        }),
        NORMAL_COMMAND_CAPACITY,
        true,
    )
    .unwrap();
    enqueue(
        &mut commands,
        Command::Engine(EngineEvent::PresentationPending {
            id,
            navigation,
            url: "https://final.example/".into(),
        }),
        NORMAL_COMMAND_CAPACITY,
        true,
    )
    .unwrap();
    enqueue(
        &mut commands,
        Command::Engine(EngineEvent::PresentationReady {
            id,
            navigation,
            url: "https://final.example/".into(),
        }),
        NORMAL_COMMAND_CAPACITY,
        true,
    )
    .unwrap();

    assert_eq!(commands.len(), 2);
    assert!(matches!(
        commands.pop_front(),
        Some(Command::Engine(EngineEvent::UrlChanged { id: observed, .. }))
            if observed == id
    ));
    assert!(matches!(
        commands.pop_front(),
        Some(Command::Engine(EngineEvent::PresentationReady {
            id: observed,
            navigation: observed_navigation,
            ..
        })) if observed == id && observed_navigation == navigation
    ));
}

#[test]
fn stale_presentation_fallback_cannot_replace_authoritative_ready_event() {
    let id = ItemId::from(7);
    let retired = NavigationPresentationId::from_raw(8);
    let current = NavigationPresentationId::from_raw(9);
    let mut commands = VecDeque::new();
    enqueue(
        &mut commands,
        Command::Engine(EngineEvent::PresentationPending {
            id,
            navigation: current,
            url: "https://current.example/".into(),
        }),
        NORMAL_COMMAND_CAPACITY,
        true,
    )
    .unwrap();
    enqueue(
        &mut commands,
        Command::Engine(EngineEvent::PresentationReady {
            id,
            navigation: current,
            url: "https://current.example/".into(),
        }),
        NORMAL_COMMAND_CAPACITY,
        true,
    )
    .unwrap();

    // Model the old wake escaping wait_for_timer and arriving after the
    // replacement navigation's Pending + Ready burst.
    enqueue(
        &mut commands,
        Command::PresentationFallback {
            id,
            navigation: retired,
            hard_deadline: std::time::Instant::now(),
        },
        NORMAL_COMMAND_CAPACITY,
        false,
    )
    .unwrap();

    assert_eq!(commands.len(), 2);
    assert!(matches!(
        commands.pop_front(),
        Some(Command::Engine(EngineEvent::PresentationReady {
            id: observed,
            navigation: observed_navigation,
            ..
        })) if observed == id && observed_navigation == current
    ));
    assert!(matches!(
        commands.pop_front(),
        Some(Command::PresentationFallback {
            id: observed,
            navigation: observed_navigation,
            ..
        }) if observed == id && observed_navigation == retired
    ));
}

#[test]
fn ordered_queue_coalesces_only_inside_an_engine_burst() {
    let id = ItemId::from(7);
    let queue = CommandQueue::new();
    queue
        .try_push(Command::Engine(EngineEvent::TitleChanged {
            id,
            title: "old".into(),
        }))
        .ok()
        .unwrap();
    queue
        .try_push(Command::Engine(EngineEvent::LoadingChanged {
            id,
            loading: true,
        }))
        .ok()
        .unwrap();
    queue
        .try_push(Command::Engine(EngineEvent::TitleChanged {
            id,
            title: "latest".into(),
        }))
        .ok()
        .unwrap();

    // Replacing a value moves it to its true latest position relative to
    // the other coalesced fields.
    assert!(matches!(
        queue.recv(),
        Some(Command::Engine(EngineEvent::LoadingChanged {
            loading: true,
            ..
        }))
    ));
    assert!(matches!(
        queue.recv(),
        Some(Command::Engine(EngineEvent::TitleChanged { title, .. })) if title == "latest"
    ));

    queue
        .try_push(Command::Engine(EngineEvent::TitleChanged {
            id,
            title: "before".into(),
        }))
        .ok()
        .unwrap();
    queue.try_push(Command::Reload(id)).ok().unwrap();
    queue
        .try_push(Command::Engine(EngineEvent::TitleChanged {
            id,
            title: "after".into(),
        }))
        .ok()
        .unwrap();

    assert!(matches!(
        queue.recv(),
        Some(Command::Engine(EngineEvent::TitleChanged { title, .. })) if title == "before"
    ));
    assert!(matches!(queue.recv(), Some(Command::Reload(value)) if value == id));
    assert!(matches!(
        queue.recv(),
        Some(Command::Engine(EngineEvent::TitleChanged { title, .. })) if title == "after"
    ));
}

#[test]
fn native_operation_facts_are_bounded_latest_per_view() {
    let id = ItemId::from(7);
    let queue = CommandQueue::new();
    for (request, applied_scale, succeeded) in [(1, 1.0, false), (2, 1.1, true), (3, 1.1, false)] {
        queue
            .try_push(Command::Engine(EngineEvent::ZoomSettled {
                id,
                request: ZoomRequestId(request),
                applied_scale,
                succeeded,
            }))
            .ok()
            .unwrap();
    }
    queue
        .try_push(Command::Engine(EngineEvent::NativeActionFailed {
            id,
            action: NativeAction::GoBack,
        }))
        .ok()
        .unwrap();
    queue
        .try_push(Command::Engine(EngineEvent::NativeActionFailed {
            id,
            action: NativeAction::GoForward,
        }))
        .ok()
        .unwrap();

    assert!(matches!(
        queue.recv(),
        Some(Command::Engine(EngineEvent::ZoomSettled {
            request: ZoomRequestId(3),
            applied_scale,
            succeeded: false,
            ..
        })) if applied_scale == 1.1
    ));
    assert!(matches!(
        queue.recv(),
        Some(Command::Engine(EngineEvent::NativeActionFailed {
            action: NativeAction::GoForward,
            ..
        }))
    ));
    assert!(queue.try_recv().is_none());
}

#[test]
fn overloaded_queue_never_blocks_and_reserves_lifecycle_capacity() {
    let id = ItemId::from(7);
    let queue = CommandQueue::new();
    for _ in 0..NORMAL_COMMAND_CAPACITY {
        queue.try_push(Command::Reload(id)).ok().unwrap();
    }
    assert!(matches!(
        queue.try_push(Command::Engine(EngineEvent::TitleChanged {
            id,
            title: "best effort".into(),
        })),
        Err(TryPushError::Full(_))
    ));
    let handle = Handle::new(queue.clone());
    assert!(!handle.dispatch(Command::Close(id)));

    // Native failure/crash transitions use a band sized for every
    // bounded recovery key. Synthetic work beyond that proven state
    // space is rejected without deleting an accepted user mutation.
    for n in 0..(LIFECYCLE_COMMAND_CAPACITY - NORMAL_COMMAND_CAPACITY) {
        queue
            .try_push(Command::Engine(EngineEvent::ViewCreationFailed {
                id: ItemId::from(100 + n as u128),
            }))
            .ok()
            .unwrap();
    }
    // A crash for an already retained physical item replaces its earlier
    // failure fact even at the hard ceiling, without consuming another slot.
    assert!(queue
        .try_push(Command::Engine(EngineEvent::Crashed {
            id: ItemId::from(100),
        }))
        .is_ok());
    assert!(matches!(
        queue.try_push(Command::Engine(EngineEvent::Crashed {
            // Stay outside the synthetic cohort as capacity changes.
            id: ItemId::from(u128::MAX),
        })),
        Err(TryPushError::Full(_))
    ));

    // One final slot belongs only to the ordered shutdown barrier. Its
    // admission atomically seals the queue, so later work is not falsely
    // reported as accepted behind a barrier that will drain it.
    let _completion = handle.shutdown();
    assert!(!handle.dispatch(Command::Open));
    let drained: Vec<_> = std::iter::from_fn(|| queue.try_recv()).collect();
    assert_eq!(drained.len(), COMMAND_QUEUE_CAPACITY);
    assert_eq!(
        drained
            .iter()
            .filter(|command| matches!(command, Command::Reload(_)))
            .count(),
        NORMAL_COMMAND_CAPACITY
    );
    assert!(matches!(drained.last(), Some(Command::Shutdown { .. })));
}

#[test]
fn blocker_completion_wake_has_profile_bounded_lifecycle_admission() {
    let queue = CommandQueue::new();
    let id = ItemId::from(7);
    for _ in 0..NORMAL_COMMAND_CAPACITY {
        queue.try_push(Command::Reload(id)).ok().unwrap();
    }
    let profile = ProfileId::from(11);
    assert!(queue.try_push(Command::BlockerReady(profile)).is_ok());
    assert!(queue.try_push(Command::BlockerReady(profile)).is_ok());
    assert!(queue.try_push(Command::BlockerStoreReady(profile)).is_ok());
    assert!(queue.try_push(Command::BlockerStoreReady(profile)).is_ok());
    let blocker_wakes = queue
        .inner
        .state
        .lock()
        .unwrap()
        .commands
        .iter()
        .filter(|command| matches!(command, Command::BlockerReady(found) if *found == profile))
        .count();
    assert_eq!(blocker_wakes, 1);
    let store_wakes = queue
        .inner
        .state
        .lock()
        .unwrap()
        .commands
        .iter()
        .filter(|command| matches!(command, Command::BlockerStoreReady(found) if *found == profile))
        .count();
    assert_eq!(store_wakes, 1);
}

#[test]
fn lifecycle_band_can_retain_a_whole_process_crash_and_shutdown() {
    let queue = CommandQueue::new();
    for value in 0..zephium_core::session::MAX_SESSION_ITEMS {
        queue
            .try_push(Command::Engine(EngineEvent::Crashed {
                id: ItemId::from(value as u128 + 1),
            }))
            .ok()
            .expect("every maximum-session view death must be admitted");
    }
    let (ack, _completion) = sync_channel(1);
    queue
        .try_push(Command::Shutdown {
            deadline: test_shutdown_deadline(),
            ack,
        })
        .ok()
        .expect("the shutdown barrier remains reserved after the crash burst");

    let drained: Vec<_> = std::iter::from_fn(|| queue.try_recv()).collect();
    assert_eq!(drained.len(), zephium_core::session::MAX_SESSION_ITEMS + 1);
    assert!(matches!(drained.last(), Some(Command::Shutdown { .. })));
}

#[test]
fn lifecycle_overload_replaces_an_old_fact_with_the_latest_same_key() {
    let id = ItemId::from(7);
    let mut commands = VecDeque::new();
    commands.push_back(Command::Engine(EngineEvent::UrlChanged {
        id,
        url: "https://old.example/".into(),
    }));
    for value in 1..LIFECYCLE_COMMAND_CAPACITY {
        commands.push_back(Command::Engine(EngineEvent::Crashed {
            id: ItemId::from(value as u128 + 10_000),
        }));
    }

    enqueue(
        &mut commands,
        Command::Engine(EngineEvent::UrlChanged {
            id,
            url: "https://latest.example/".into(),
        }),
        LIFECYCLE_COMMAND_CAPACITY,
        true,
    )
    .expect("latest bounded native fact must replace its stale predecessor");

    assert_eq!(commands.len(), LIFECYCLE_COMMAND_CAPACITY);
    assert!(matches!(
        commands.back(),
        Some(Command::Engine(EngineEvent::UrlChanged { id: observed, url }))
            if *observed == id && url == "https://latest.example/"
    ));
    assert!(!commands.iter().any(|command| matches!(
        command,
        Command::Engine(EngineEvent::UrlChanged { url, .. })
            if url == "https://old.example/"
    )));
}

#[test]
fn critical_overload_never_evicts_an_accepted_user_mutation() {
    let id = ItemId::from(7);
    let window: WindowId = 1;
    let mut commands = VecDeque::new();
    commands.push_back(Command::Engine(EngineEvent::UrlChanged {
        id,
        url: "https://committed.example/".into(),
    }));
    for _ in 1..LIFECYCLE_COMMAND_CAPACITY {
        commands.push_back(Command::Reload(id));
    }

    assert!(enqueue(
        &mut commands,
        Command::Engine(EngineEvent::SplitChanged {
            window,
            tree: Pane::Leaf(id),
        }),
        LIFECYCLE_COMMAND_CAPACITY,
        true,
    )
    .is_err());

    assert!(commands.iter().any(|command| matches!(
        command,
        Command::Engine(EngineEvent::UrlChanged { url, .. })
            if url == "https://committed.example/"
    )));
    assert_eq!(
        commands
            .iter()
            .filter(|command| matches!(command, Command::Reload(_)))
            .count(),
        LIFECYCLE_COMMAND_CAPACITY - 1
    );
    assert!(!commands
        .iter()
        .any(|command| matches!(command, Command::Engine(EngineEvent::SplitChanged { .. }))));
    assert_eq!(commands.len(), LIFECYCLE_COMMAND_CAPACITY);
}

#[test]
fn ordinary_overload_evicts_only_ordinary_presentation_state() {
    let id = ItemId::from(7);
    let mut commands = VecDeque::new();
    commands.push_back(Command::Engine(EngineEvent::UrlChanged {
        id,
        url: "https://committed.example/".into(),
    }));
    commands.push_back(Command::Engine(EngineEvent::TitleChanged {
        id,
        title: "stale".into(),
    }));
    for _ in commands.len()..NORMAL_COMMAND_CAPACITY {
        commands.push_back(Command::Reload(id));
    }

    enqueue(
        &mut commands,
        Command::Close(id),
        NORMAL_COMMAND_CAPACITY,
        false,
    )
    .expect("ordinary intent may displace ordinary presentation state");
    assert!(commands
        .iter()
        .any(|command| matches!(command, Command::Engine(EngineEvent::UrlChanged { .. }))));
    assert!(!commands
        .iter()
        .any(|command| matches!(command, Command::Engine(EngineEvent::TitleChanged { .. }))));
    assert!(commands
        .iter()
        .any(|command| matches!(command, Command::Close(value) if *value == id)));
}

#[test]
fn observational_status_query_never_evicts_browser_state_at_capacity() {
    let id = ItemId::from(7);
    let queue = CommandQueue::new();
    assert!(queue
        .try_push(Command::Engine(EngineEvent::TitleChanged {
            id,
            title: "retained presentation".into(),
        }))
        .is_ok());
    for _ in 1..NORMAL_COMMAND_CAPACITY {
        assert!(queue.try_push(Command::Reload(id)).is_ok());
    }

    let (reply, _receiver) = sync_channel(1);
    assert!(matches!(
        queue.try_push(Command::FocusedContentPolicyStatus { reply }),
        Err(TryPushError::Full(
            Command::FocusedContentPolicyStatus { .. }
        ))
    ));
    let state = queue.inner.state.lock().unwrap();
    assert_eq!(state.commands.len(), NORMAL_COMMAND_CAPACITY);
    assert!(state.commands.iter().any(|command| matches!(
        command,
        Command::Engine(EngineEvent::TitleChanged { title, .. })
            if title == "retained presentation"
    )));
}
