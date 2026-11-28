wed(&caller, "page_assist_resume")
        && valid_id(&conversation)
        && with_service!(false, |service| service.resume(&conversation, reply))
}

/// Starts the Work a reply offered.
#[tauri::command]
#[specta::specta]
pub(crate) async fn page_assist_continue(
    caller: WebviewWindow,
    conversation: String,
    reply: u32,
    block: u16,
) -> PageAssistContinueV1 {
    let unavailable = PageAssistContinueV1 {
        work: None,
        fault: Some(PageAssistFaultV1::Unavailable),
    };
    if !allowed(&caller, "page_assist_continue") || !valid_id(&conversation) {
        return unavailable;
    }
    with_service!(unavailable, |service| service
        .continue_work(&conversation, reply, block)
        .await)
}

/// Starts a new conversation in the Space; the old one stays in history.
#[tauri::command]
#[specta::specta]
pub(crate) async fn page_assist_new(caller: WebviewWindow, space: String) -> bool {
    allowed(&caller, "page_assist_new")
        && valid_space(&space)
        && with_service!(false, |service| service.new_conversation(&space))
}

/// Past conversations of the Space's profile, newest first.
#[tauri::command]
#[specta::specta]
pub(crate) async fn page_assist_history(
    caller: WebviewWindow,
    space: String,
) -> PageAssistHistoryV1 {
    let empty = PageAssistHistoryV1 {
        version: VERSION,
        conversations: Vec::new(),
    };
    if !allowed(&caller, "page_assist_history") || !valid_space(&space) {
        return empty;
    }
    with_service!(empty, |service| service.history(&space))
}

/// Shows a past conversation in the Space.
#[tauri::command]
#[specta::specta]
pub(crate) async fn page_assist_open(
    caller: WebviewWindow,
    space: String,
    conversation: String,
) -> bool {
    allowed(&caller, "page_assist_open")
        && valid_space(&space)
        && valid_id(&conversation)
        && with_service!(false, |service| service.open(&space, &conversation))
}

/// Deletes a conversation.
#[tauri::command]
#[specta::specta]
pub(crate) async fn page_assist_delete(
    caller: WebviewWindow,
    space: String,
    conversation: String,
) -> bool {
    allowed(&caller, "page_assist_delete")
        && valid_space(&space)
        && valid_id(&conversation)
        && with_service!(false, |service| service.delete(&space, &conversation))
}

/// The person picks an element on the tab; resolves when they pick or cancel.
#[tauri::command]
#[specta::specta]
pub(crate) async fn page_assist_pick(
    caller: WebviewWindow,
    space: String,
    tab: String,
) -> PageAssistPickResponseV1 {
    let unavailable = PageAssistPickResponseV1 {
        pick: None,
        fault: Some(PageAssistFaultV1::Unavailable),
    };
    if !allowed(&caller, "page_assist_pick") || !valid_space(&space) || !valid_tab(&tab) {
        return unavailable;
    }
    with_service!(unavailable, |service| service.pick(&space, &tab).await)
}

/// Ends a pick in progress.
#[tauri::command]
#[specta::specta]
pub(crate) async fn page_assist_pick_cancel(caller: WebviewWindow, tab: String) -> bool {
    allowed(&caller, "page_assist_pick_cancel")
        && valid_tab(&tab)
        && with_service!(false, |service| {
            service.pick_cancel(&tab);
            true
        })
}

/// Detaches a picked element and removes its outline.
#[tauri::command]
#[specta::specta]
pub(crate) async fn page_assist_detach(caller: WebviewWindow, tab: String, pick: String) -> bool {
    allowed(&caller, "page_assist_detach")
        && valid_tab(&tab)
        && valid_id(&pick)
        && with_service!(false, |service| service.detach(&tab, &pick))
}

/// The panel closed: marks and picks leave the Space's pages.
#[tauri::command]
#[specta::specta]
pub(crate) async fn page_assist_close(caller: WebviewWindow, space: String) -> bool {
    allowed(&caller, "page_assist_close")
        && valid_space(&space)
        && with_service!(false, |service| {
            service.close(&space);
            true
        })
}

/// The page styles of the site in `tab`.
#[tauri::command]
#[specta::specta]
pub(crate) async fn page_styles(caller: WebviewWindow, tab: String) -> PageStylesV1 {
    if !allowed(&caller, "page_styles") {
        return no_styles(tab);
    }
    with_service!(no_styles(tab), |service| service.styles(&tab))
}

/// Turns one of the site's styles on or off.
#[tauri::command]
#[specta::specta]
pub(crate) async fn page_style_toggle(
    caller: WebviewWindow,
    tab: String,
    style: String,
    enabled: bool,
) -> PageStylesV1 {
    if !allowed(&caller, "page_style_toggle") || !valid_id(&style) {
        return no_styles(tab);
    }
    with_service!(no_styles(tab), |service| service
        .toggle_style(&tab, &style, enabled))
}

/// Deletes one of the site's styles.
#[tauri::command]
#[specta::specta]
pub(crate) async fn page_style_remove(
    caller: WebviewWindow,
    tab: String,
    style: String,
) -> PageStylesV1 {
    if !allowed(&caller, "page_style_remove") || !valid_id(&style) {
        return no_styles(tab);
    }
    with_service!(no_styles(tab), |service| service.remove_style(&tab, &style))
}

#[cfg(feature = "work-product")]
fn profile_of(profile: &str) -> Option<zephium_core::ids::ProfileId> {
    zephium_core::ids::ProfileId::parse(profile).filter(|id| id.to_string() == profile)
}

/// Every site's page styles in `profile`, for Settings.
#[tauri::command]
#[specta::specta]
pub(crate) async fn page_styles_all(caller: WebviewWindow, profile: String) -> PageStylesAllV1 {
    if !allowed(&caller, "page_styles_all") {
        return no_profile_styles(profile);
    }
    with_service!(no_profile_styles(profile), |service| {
        match profile_of(&profile) {
            Some(id) => service.all_styles(&id),
            None => no_profile_styles(profile),
        }
    })
}

/// Turns one of a site's styles on or off from Settings.
#[tauri::command]
#[specta::specta]
pub(crate) async fn page_style_toggle_for(
    caller: WebviewWindow,
    profile: String,
    site: String,
    style: String,
    enabled: bool,
) -> PageStylesAllV1 {
    if !allowed(&caller, "page_style_toggle_for") || !valid_id(&style) || site.len() > 253 {
        return no_profile_styles(profile);
    }
    with_service!(no_profile_styles(profile), |service| {
        match profile_of(&profile) {
            Some(id) => service.toggle_style_for(&id, &site, &style, enabled),
            None => no_profile_styles(profile),
        }
    })
}

/// Deletes one of a site's styles from Settings.
#[tauri::command]
#[specta::specta]
pub(crate) async fn page_style_remove_for(
    caller: WebviewWindow,
    profile: String,
    site: String,
    style: String,
) -> PageStylesAllV1 {
    if !allowed(&caller, "page_style_remove_for") || !valid_id(&style) || site.len() > 253 {
        return no_profile_styles(profile);
    }
    with_service!(no_profile_styles(profile), |service| {
        match profile_of(&profile) {
            Some(id) => service.remove_style_for(&id, &site, &style),
            None => no_profile_styles(profile),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_and_style_ids_are_closed() {
        assert!(valid_tab("01HZX3J5Q4F0Y8N9W2E6R7T1M3"));
        for bad in ["", "a b", "../x", &"a".repeat(65)] {
            assert!(!valid_tab(bad), "{bad}");
        }
        assert!(valid_id("s123-4"));
        assert!(!valid_id("s1\"};"));
    }
}

/// "Run in Work" from the Ask panel: the same Work documents and operation
/// owner the panel's own Work flow uses, in order.
#[cfg(feature = "work-product")]
mod handoff {
    use super::*;
    use std::time::Duration;
    use zephium_core::ids::{ItemId, ProfileId, SpaceId};
    use zephium_core::work::authoring::WorkAuthoringIntent;
    use zephium_core::work::context::{
        url_revision, WorkContextSelectionItem, WorkContextSelectionV1,
    };
    use zephium_core::work::environment::{
        WorkEnvironmentCall, WorkEnvironmentEdit, WorkEnvironmentIntent, WorkEnvironmentReference,
        WorkEnvironmentReply, WorkEnvironmentSnapshot,
    };
    use zephium_core::work::runtime::WorkRuntimeIntent;
    use zephium_core::work::{WorkCommandId, WorkError};
    use zephium_ipc::work::{
        WorkAuthoringCommandV1, WorkCallV1, WorkCommandV1, WorkOperationStateV1, WorkOperationV1,
        WorkReplyV1,
    };

    const TIMEOUT: Duration = Duration::from_secs(8);
    const MAX_TITLE_CHARS: usize = 80;

    async fn call(
        app: &tauri::AppHandle,
        profile: ProfileId,
        call: WorkCallV1,
    ) -> Result<WorkReplyV1, WorkError> {
        let request = app
            .state::<zephium_app::Handle>()
            .work_call(profile, call)?;
        let response = tokio::time::timeout(TIMEOUT, request.response(profile))
            .await
            .map_err(|_| WorkError::OutcomeUnknown)?;
        match response.reply {
            WorkReplyV1::Error { .. } => Err(WorkError::Unavailable),
            reply => Ok(reply),
        }
    }

    fn environment(intent: WorkEnvironmentIntent) -> WorkCallV1 {
        WorkCallV1::Environment {
            version: 1,
            request: WorkEnvironmentCall::Command {
                command: WorkCommandId::generate(),
                intent,
            },
        }
    }

    fn added(
        snapshot: &WorkEnvironmentSnapshot,
        reference: WorkEnvironmentReference,
    ) -> WorkEnvironmentIntent {
        WorkEnvironmentIntent::Edit {
            id: snapshot.id,
            expected: snapshot.revision,
            edit: WorkEnvironmentEdit::Add {
                reference,
                area: None,
            },
        }
    }

    fn applied(reply: WorkReplyV1) -> Result<Box<WorkEnvironmentSnapshot>, WorkError> {
        match reply {
            WorkReplyV1::Environment {
                reply: WorkEnvironmentReply::Applied { snapshot, .. },
            } => Ok(snapshot),
            _ => Err(WorkError::Unavailable),
        }
    }

    /// The panel's own routine grant and limits (`AGENT_GRANT`, `AGENT_LIMITS`
    /// in `frame/src/domain/work/work-model.ts`).
    pub(super) fn begin_agent() -> Result<WorkRuntimeIntent, WorkError> {
        serde_json::from_value(serde_json::json!({
            "kind": "begin_agent",
            "grant": {
                "provider": "open_ai",
                "model": "gpt-6-luna",
                "max_turns": 10,
                "max_steps": 32,
                "browse_hops": 4
            },
            "limits": {
                "model_tokens": 1_000_000,
                "cost_micro_usd": 3_000_000,
                "operations": 256,
                "timeout_seconds": 1800,
                "max_workers": 4
            }
        }))
        .map_err(|_| WorkError::Invalid)
    }

    /// Creates a Work in the Space with the handoff's tabs and objective on
    /// its canvas, then begins a run with those tabs as context.
    pub(super) async fn start(
        app: &tauri::AppHandle,
        profile: ProfileId,
        space: &str,
        start: &zephium_work_composition::page_assist::port::WorkStart,
        tabs: &[(String, String)],
    ) -> Result<String, WorkError> {
        let space = SpaceId::parse(space).ok_or(WorkError::Invalid)?;
        let intent = begin_agent()?;
        let title = zephium_work_composition::page_assist::core::text::bounded(
            &zephium_work_composition::page_assist::core::text::clean(&start.title),
            MAX_TITLE_CHARS,
        );
        let mut snapshot = applied(
            call(
                app,
                profile,
                environment(WorkEnvironmentIntent::Create { space, title }),
            )
            .await?,
        )?;
        let mut items = Vec::new();
        for (tab, url) in tabs {
            let Some(tab) = ItemId::parse(tab) else {
                continue;
            };
            let browser = WorkEnvironmentReference::Browser { tab };
            snapshot =
                applied(call(app, profile, environment(added(&snapshot, browser.clone()))).await?)?;
            if let Some(element) = snapshot
                .elements
                .iter()
                .find(|element| element.reference == browser)
            {
                items.push(WorkContextSelectionItem {
                    element: element.id,
                    revision: url_revision(url),
                });
            }
        }
        let WorkReplyV1::AuthoringApplied { receipt } = call(
            app,
            profile,
            WorkCallV1::Author {
                command: WorkAuthoringCommandV1 {
                    version: 1,
                    command: WorkCommandId::generate(),
                    intent: WorkAuthoringIntent::Create {
                        objective: start.objective.clone(),
                    },
                },
            },
        )
        .await?
        else {
            return Err(WorkError::Unavailable);
        };
        // The objective joins the canvas beside its tabs; the run does not
        // depend on it, so a refusal here does not stop the run.
        let objective = WorkEnvironmentReference::Objective {
            objective: receipt.work,
        };
        let _ = call(app, profile, environment(added(&snapshot, objective))).await;
        let operation = WorkCommandId::generate();
        let input = WorkOperationV1::Run {
            command: WorkCommandV1 {
                version: 1,
                work: receipt.work,
                expected_revision: receipt.applied_revision,
                command: WorkCommandId::generate(),
                intent,
            },
            context: (!items.is_empty()).then_some(WorkContextSelectionV1 {
                environment: snapshot.id,
                items,
                tabs: false,
                blocks: false,
            }),
            signed_in: Default::default(),
        };
        match super::super::work_product::admit_run(app, profile, operation, input)? {
            WorkOperationStateV1::Refused { .. } => Err(WorkError::Unavailable),
            _ => Ok(receipt.work.to_string()),
        }
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn the_grant_matches_the_panels_own() {
            assert!(super::begin_agent().is_ok());
            let frame = include_str!("../../frame/src/domain/work/work-model.ts");
            for pinned in [
                "provider: \"open_ai\"",
                "model: \"gpt-6-luna\"",
                "max_turns: 10",
                "max_steps: 32",
                "browse_hops: 4",
                "model_tokens: 1_000_000",
                "cost_micro_usd: 3_000_000",
                "operations: 256",
                "timeout_seconds: 1800",
                "max_workers: 4",
            ] {
                assert!(
                    frame.contains(pinned),
                    "{pinned} drifted from the panel's grant"
                );
            }
        }
    }
}
