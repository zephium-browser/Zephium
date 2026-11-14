use super::*;
use protocol::{
    Evaluation, FrameEvent, InjectRequest, InjectionKind, InjectionResult, InjectionStatus,
    InjectionTarget, RunAt,
};

const MAX_PENDING_INJECTIONS: usize = 64;
const MAX_RESULT_BYTES: usize = 1024 * 1024;

#[derive(Clone)]
struct Document {
    page: u64,
    frame: u64,
    sequence: u64,
}

pub(super) struct PendingInjection {
    page: u64,
    generation: u64,
    request_id: u64,
    frame_id: u64,
    document_seq: u64,
    world: String,
    timeout_source: u32,
}

pub(super) fn receiver_key(world: &str, install: Option<&str>) -> String {
    install.map_or_else(|| world.to_owned(), |install| format!("{world}\0{install}"))
}

fn context(page: u64, frame: u64, world: &str) -> Option<Owned> {
    let native = with_runtime(|runtime| {
        runtime
            .pages
            .get(&page)?
            .frames
            .get(&frame)
            .map(|frame| frame.pointer)
    })
    .flatten()?;
    let frame = Owned::borrowed(native)?;
    let world = ensure_world(world)?;
    Owned::full(unsafe { raw::webkit_frame_get_js_context_for_script_world(frame.0, world) })
}

fn evaluate(context: &Owned, source: &str, url: &str) -> (Option<Owned>, bool) {
    let Ok(url) = CString::new(url) else {
        return (None, false);
    };
    unsafe {
        let value = Owned::full(raw::jsc_context_evaluate_with_source_uri(
            context.0,
            source.as_ptr().cast(),
            source.len() as isize,
            url.as_ptr(),
            1,
        ));
        let success = raw::jsc_context_get_exception(context.0).is_null();
        raw::jsc_context_clear_exception(context.0);
        (value, success)
    }
}

fn live(document: &Document) -> bool {
    with_runtime(|runtime| {
        runtime
            .state
            .frame(document.page, document.frame)
            .is_some_and(|frame| frame.document_seq == document.sequence)
    })
    .unwrap_or(false)
}

fn entry_admitted(document: &Document, entry: &protocol::ScriptEntry) -> bool {
    with_runtime(|runtime| {
        let Some(info) = runtime.state.frame(document.page, document.frame) else {
            return false;
        };
        info.document_seq == document.sequence
            && entry.rule.matches(info)
            && runtime
                .state
                .attachment(document.page)
                .is_some_and(|attach| {
                    attach.worlds.iter().any(|world| {
                        world.install_id == entry.install_id
                            && (entry.world.is_empty() || world.name == entry.world)
                            && world.matches.iter().any(|rule| rule.matches(info))
                    })
                })
    })
    .unwrap_or(false)
}

pub(super) fn collect_facts(page: u64, frame: u64) {
    let Some(child) = context(page, frame, FACT_WORLD) else {
        return;
    };
    let (window, _) = evaluate(&child, "window", "zephium://native/frame-facts");
    let (origin, _) = evaluate(&child, "globalThis.origin", "zephium://native/frame-facts");
    let origin = origin.and_then(|value| super::form_string(value.0, protocol::MAX_URI_BYTES));
    let candidates = with_runtime(|runtime| {
        runtime.pages.get(&page).map(|page| {
            page.frames
                .keys()
                .copied()
                .filter(|candidate| *candidate != frame)
                .collect::<Vec<_>>()
        })
    })
    .flatten()
    .unwrap_or_default();
    let mut parent = None;
    if let Some(window) = window {
        for candidate in candidates {
            let Some(parent_context) = context(page, candidate, FACT_WORLD) else {
                continue;
            };
            let (helper, success) = evaluate(&parent_context, "(child => { for (let i = 0; i < Math.min(frames.length, 128); ++i) if (frames[i] === child) return true; return false; })", "zephium://native/frame-facts");
            if !success {
                continue;
            }
            let Some(helper) = helper else {
                continue;
            };
            let mut argument = window.0;
            let result =
                Owned::full(unsafe { raw::jsc_value_function_callv(helper.0, 1, &mut argument) });
            let found =
                result.is_some_and(|result| unsafe { raw::jsc_value_to_boolean(result.0) != 0 });
            unsafe {
                raw::jsc_context_clear_exception(parent_context.0);
            }
            if found {
                parent = Some(candidate);
                break;
            }
        }
    }
    with_runtime(|runtime| {
        let Some(info) = runtime.state.frame(page, frame) else {
            return;
        };
        let opaque =
            origin.as_deref().is_none_or(|value| value == "null") || info.uri.starts_with("data:");
        let creator = parent
            .and_then(|parent| runtime.state.frame(page, parent))
            .and_then(|info| {
                // Only report a precursor supported by the actual document origin.
                (protocol::document_origin(&info.uri).as_deref() == origin.as_deref())
                    .then(|| info.uri.clone())
            });
        runtime
            .state
            .set_frame_facts(page, frame, parent, creator, opaque);
    });
}

pub(super) fn install_bridge(binding: Binding, install_id: &str, bootstrap: &str) -> bool {
    let page = binding.page;
    let frame = binding.frame;
    let sequence = binding.document_seq;
    let world = binding.world.as_str();
    let key = receiver_key(world, binding.external_install.as_deref());
    let first = with_runtime(|runtime| {
        runtime
            .pages
            .get_mut(&page)?
            .frames
            .get_mut(&frame)
            .map(|frame| frame.installed.insert(key.clone()))
    })
    .flatten()
    .unwrap_or(false);
    let Some(context) = context(page, frame, world) else {
        return false;
    };
    if first {
        install_function(
            &context,
            protocol::POST_FUNCTION,
            post_bytes as *const (),
            binding.clone(),
        );
        install_function(
            &context,
            protocol::REGISTER_FUNCTION,
            register_receiver as *const (),
            binding.clone(),
        );
    }
    if bootstrap.is_empty() {
        return true;
    }
    if let Some(outcome) = with_runtime(|runtime| {
        runtime
            .pages
            .get(&page)?
            .frames
            .get(&frame)?
            .bootstrapped
            .get(&key)
            .copied()
    })
    .flatten()
    {
        return outcome;
    }
    with_runtime(|runtime| {
        if let Some(frame) = runtime
            .pages
            .get_mut(&page)
            .and_then(|page| page.frames.get_mut(&frame))
        {
            frame.bootstrapped.insert(key.clone(), false);
        }
    });
    let charged = with_runtime(|runtime| {
        runtime
            .state
            .charge_script_bytes(page, frame, sequence, bootstrap.len())
            .is_ok()
    })
    .unwrap_or(false);
    let success = charged && evaluate(&context, bootstrap, "zephium://native/bootstrap").1;
    with_runtime(|runtime| {
        if let Some(frame) = runtime
            .pages
            .get_mut(&page)
            .and_then(|page| page.frames.get_mut(&frame))
        {
            frame.bootstrapped.insert(key, success);
        }
    });
    if !install_id.is_empty() {
        with_runtime(|runtime| {
            runtime.state.queue_frame_event(
                page,
                FrameEvent::Evaluated(Evaluation {
                    install_id: install_id.to_owned(),
                    script_id: String::new(),
                    world: world.to_owned(),
                    frame_id: frame,
                    document_seq: sequence,
                    table_generation: 0,
                    file_index: u16::MAX,
                    success,
                }),
            )
        });
        flush_frame_events(page);
    }
    success
}

pub(super) fn install_external(page: u64, frame: u64, sequence: u64, _generation: u64) {
    let rules = with_runtime(|runtime| {
        let info = runtime.state.frame(page, frame)?;
        Some(
            runtime
                .state
                .attachment(page)?
                .external
                .iter()
                .filter(|rule| rule.admits(info))
                .cloned()
                .collect::<Vec<_>>(),
        )
    })
    .flatten()
    .unwrap_or_default();
    for rule in rules {
        // The supplied bootstrap captures a separate native callback for its install.
        if install_bridge(
            Binding {
                page,
                frame,
                document_seq: sequence,
                world: String::new(),
                external_install: Some(rule.install_id.clone()),
            },
            "",
            &rule.bootstrap,
        ) {
            with_runtime(|runtime| {
                runtime.state.queue_frame_event(
                    page,
                    FrameEvent::ExternalPage {
                        install_id: rule.install_id,
                        frame_id: frame,
                        document_seq: sequence,
                    },
                )
            });
            flush_frame_events(page);
        }
    }
}

pub(super) fn run_scripts(page: u64, frame: u64, sequence: u64, phase: RunAt) {
    let document = Document {
        page,
        frame,
        sequence,
    };
    let index = match phase {
        RunAt::DocumentStart => 0,
        RunAt::DocumentEnd => 1,
        RunAt::DocumentIdle => 2,
    };
    let work = with_runtime(|runtime| {
        let info = runtime.state.frame(page, frame)?.clone();
        if info.document_seq != sequence {
            return None;
        }
        let native = runtime.pages.get_mut(&page)?.frames.get_mut(&frame)?;
        if native.phases[index] {
            return None;
        }
        native.phases[index] = true;
        let table = native.scripts.clone()?;
        let attach = runtime.state.attachment(page)?;
        let entries = table
            .entries
            .iter()
            .filter(|entry| {
                entry.run_at == phase
                    && entry.rule.matches(&info)
                    && attach.worlds.iter().any(|world| {
                        world.install_id == entry.install_id
                            && (entry.world.is_empty() || world.name == entry.world)
                            && world.matches.iter().any(|rule| rule.matches(&info))
                    })
            })
            .cloned()
            .collect::<Vec<_>>();
        Some((table.generation, entries))
    })
    .flatten();
    let Some((table_generation, entries)) = work else {
        return;
    };
    if !flush_frame_events(page) {
        return;
    }
    'entries: for entry in entries {
        if with_runtime(|runtime| runtime.state.script_budget_exhausted(page, frame, sequence))
            .unwrap_or(true)
        {
            break;
        }
        if !entry_admitted(&document, &entry) {
            if !live(&document) {
                break;
            }
            continue;
        }
        let Some(context) = context(page, frame, &entry.world) else {
            continue;
        };
        if !entry.world.is_empty() {
            if !install_bridge(
                Binding {
                    page,
                    frame,
                    document_seq: sequence,
                    world: entry.world.clone(),
                    external_install: None,
                },
                "",
                "",
            ) {
                continue;
            }
            let outcome = with_runtime(|runtime| {
                runtime
                    .pages
                    .get(&page)?
                    .frames
                    .get(&frame)?
                    .bootstrapped
                    .get(&entry.world)
                    .copied()
            })
            .flatten();
            if outcome == Some(false) {
                continue;
            }
            if outcome.is_none() && !entry.bootstrap.is_empty() {
                with_runtime(|runtime| {
                    if let Some(frame) = runtime
                        .pages
                        .get_mut(&page)
                        .and_then(|page| page.frames.get_mut(&frame))
                    {
                        frame.bootstrapped.insert(entry.world.clone(), false);
                    }
                });
                let charged = with_runtime(|runtime| {
                    runtime
                        .state
                        .charge_script_bytes(page, frame, sequence, entry.bootstrap.len())
                        .is_ok()
                })
                .unwrap_or(false);
                let success = charged
                    && evaluate(
                        &context,
                        &entry.bootstrap,
                        "zephium://native/content-bootstrap",
                    )
                    .1;
                with_runtime(|runtime| {
                    if let Some(frame) = runtime
                        .pages
                        .get_mut(&page)
                        .and_then(|page| page.frames.get_mut(&frame))
                    {
                        frame.bootstrapped.insert(entry.world.clone(), success);
                    }
                    runtime.state.queue_frame_event(
                        page,
                        FrameEvent::Evaluated(Evaluation {
                            install_id: entry.install_id.clone(),
                            script_id: entry.script_id.clone(),
                            world: entry.world.clone(),
                            frame_id: frame,
                            document_seq: sequence,
                            table_generation,
                            file_index: u16::MAX,
                            success,
                        }),
                    );
                });
                if !flush_frame_events(page) || !charged {
                    break;
                }
                if !success {
                    continue;
                }
            }
        }
        for (file_index, file) in entry.files.iter().enumerate() {
            if !entry_admitted(&document, &entry) {
                break;
            }
            let charged = with_runtime(|runtime| {
                runtime
                    .state
                    .charge_script_bytes(page, frame, sequence, file.source.len())
                    .is_ok()
            })
            .unwrap_or(false);
            let success = charged && evaluate(&context, &file.source, &file.url).1;
            with_runtime(|runtime| {
                runtime.state.queue_frame_event(
                    page,
                    FrameEvent::Evaluated(Evaluation {
                        install_id: entry.install_id.clone(),
                        script_id: entry.script_id.clone(),
                        world: entry.world.clone(),
                        frame_id: frame,
                        document_seq: sequence,
                        table_generation,
                        file_index: file_index as u16,
                        success,
                    }),
                )
            });
            if !flush_frame_events(page) || !charged {
                break 'entries;
            }
        }
    }
}

pub(super) fn install_lifecycle(page: u64, frame: u64, sequence: u64) {
    let Some(context) = context(page, frame, FACT_WORLD) else {
        return;
    };
    let document = Document {
        page,
        frame,
        sequence,
    };
    unsafe {
        let mut value_type = raw::jsc_value_get_type();
        let data = Box::into_raw(Box::new(document));
        let Some(callback_value) = Owned::full(raw::jsc_value_new_functionv(
            context.0,
            c"lifecycle".as_ptr(),
            callback(lifecycle as *const ()),
            data.cast(),
            Some(document_destroyed),
            G_TYPE_BOOLEAN,
            1,
            &mut value_type,
        )) else {
            drop(Box::from_raw(data));
            return;
        };
        let (helper, success) = evaluate(
            &context,
            r#"(native => {
    const listen = EventTarget.prototype.addEventListener;
    const remove = EventTarget.prototype.removeEventListener;
    const end = event => {
        if (!event.isTrusted || event.target !== document) return;
        remove.call(window, 'DOMContentLoaded', end, true); native(1);
    };
    const idle = event => {
        if (!event.isTrusted || event.target !== document) return;
        remove.call(window, 'load', idle, true); native(2);
    };
    listen.call(window, 'DOMContentLoaded', end, true);
    listen.call(window, 'load', idle, true);
})"#,
            "zephium://native/lifecycle",
        );
        if !success {
            return;
        }
        if let Some(helper) = helper {
            let mut argument = callback_value.0;
            drop(Owned::full(raw::jsc_value_function_callv(
                helper.0,
                1,
                &mut argument,
            )));
            raw::jsc_context_clear_exception(context.0);
        }
    }
}

unsafe extern "C" fn document_destroyed(data: *mut c_void) {
    boundary(|| unsafe {
        drop(Box::from_raw(data.cast::<Document>()));
    });
}
unsafe extern "C" fn lifecycle(value: *mut Object, data: *mut c_void) -> c_int {
    boundary(|| unsafe {
        let document = &*data.cast::<Document>();
        if !live(document) {
            return 0;
        }
        match raw::jsc_value_to_int32(value) {
            1 => {
                run_scripts(
                    document.page,
                    document.frame,
                    document.sequence,
                    RunAt::DocumentEnd,
                );
                raw::g_timeout_add_full(
                    0,
                    200,
                    Some(idle),
                    Box::into_raw(Box::new(document.clone())).cast(),
                    Some(document_destroyed),
                );
            }
            2 => {
                run_scripts(
                    document.page,
                    document.frame,
                    document.sequence,
                    RunAt::DocumentEnd,
                );
                run_scripts(
                    document.page,
                    document.frame,
                    document.sequence,
                    RunAt::DocumentIdle,
                );
            }
            _ => return 0,
        }
        1
    })
}
unsafe extern "C" fn idle(data: *mut c_void) -> c_int {
    boundary(|| unsafe {
        let document = &*data.cast::<Document>();
        if live(document) {
            run_scripts(
                document.page,
                document.frame,
                document.sequence,
                RunAt::DocumentIdle,
            );
        }
        0
    })
}

fn validated_result(mut result: InjectionResult) -> InjectionResult {
    if result.status == InjectionStatus::Success
        && protocol::encode_event(&Event::Injection(result.clone())).is_err()
    {
        result.status = InjectionStatus::EvaluationFailed;
        result.result_json = "null".to_owned();
    }
    result
}

fn publish_result(pending: PendingInjection, mut status: InjectionStatus, result_json: String) {
    let current = with_runtime(|runtime| {
        runtime
            .state
            .attachment(pending.page)
            .is_some_and(|attach| attach.generation == pending.generation)
            && runtime
                .state
                .frame(pending.page, pending.frame_id)
                .is_some_and(|frame| frame.document_seq == pending.document_seq)
    })
    .unwrap_or(false);
    if !current {
        status = InjectionStatus::StaleTarget;
    }
    send_event_immediately(
        pending.page,
        Event::Injection(validated_result(InjectionResult {
            generation: pending.generation,
            request_id: pending.request_id,
            frame_id: pending.frame_id,
            document_seq: pending.document_seq,
            world: pending.world,
            status,
            result_json: if status == InjectionStatus::Success {
                result_json
            } else {
                "null".to_owned()
            },
        })),
    );
}

fn finish(id: u64, status: InjectionStatus, result_json: String) {
    if let Some(pending) = with_runtime(|runtime| runtime.injections.remove(&id)).flatten() {
        if pending.timeout_source != 0 {
            unsafe {
                raw::g_source_remove(pending.timeout_source);
            }
        }
        publish_result(pending, status, result_json);
    }
}

unsafe extern "C" fn injection_id_destroyed(data: *mut c_void) {
    boundary(|| unsafe {
        drop(Box::from_raw(data.cast::<u64>()));
    });
}
unsafe extern "C" fn injection_timeout(data: *mut c_void) -> c_int {
    boundary(|| unsafe {
        finish(
            *data.cast::<u64>(),
            InjectionStatus::TimedOut,
            "null".to_owned(),
        );
        0
    })
}

unsafe extern "C" fn injection_complete(value: *mut Object, data: *mut c_void) -> c_int {
    boundary(|| unsafe {
        let id = *data.cast::<u64>();
        if value.is_null()
            || raw::jsc_value_is_typed_array(value) == 0
            || raw::jsc_value_typed_array_get_type(value) != UINT8_ARRAY
        {
            finish(id, InjectionStatus::EvaluationFailed, "null".into());
            return 0;
        }
        let length = raw::jsc_value_typed_array_get_size(value);
        if length == 0 || length > MAX_RESULT_BYTES + 1 {
            finish(id, InjectionStatus::Oversized, "null".into());
            return 0;
        }
        let mut actual = 0;
        let pointer = raw::jsc_value_typed_array_get_data(value, &mut actual);
        if pointer.is_null() || actual != length {
            finish(id, InjectionStatus::EvaluationFailed, "null".into());
            return 0;
        }
        let bytes = std::slice::from_raw_parts(pointer.cast::<u8>(), length);
        let status = match bytes[0] {
            0 => InjectionStatus::Success,
            1 => InjectionStatus::EvaluationFailed,
            2 => InjectionStatus::Oversized,
            _ => InjectionStatus::EvaluationFailed,
        };
        let result = std::str::from_utf8(&bytes[1..])
            .unwrap_or("null")
            .to_owned();
        finish(id, status, result);
        1
    })
}

// Capture serialization primitives in the private native world before invoking
// extension code. Only a capped typed array crosses into the native callback.
const AWAIT_RESULT: &str = r#"((value, done) => {
    const resolve = Promise.resolve.bind(Promise), stringify = JSON.stringify;
    const Encoder = TextEncoder, Array = Uint8Array;
    const fail = code => done(new Array([code,110,117,108,108]));
    resolve(value).then(result => {
        try {
            let json = stringify(result);
            if (json === undefined) json = 'null';
            if (json.length > 1048576) { fail(2); return; }
            const bytes = new Encoder().encode(json);
            if (bytes.length > 1048576) { fail(2); return; }
            const output = new Array(bytes.length + 1); output.set(bytes, 1); done(output);
        } catch (_) { fail(1); }
    }, () => fail(1));
})"#;

fn prepare_injection_world(page: u64, frame: u64, sequence: u64, request: &InjectRequest) -> bool {
    if request.world.is_empty() {
        return true;
    }
    let bootstrap = with_runtime(|runtime| {
        let info = runtime.state.frame(page, frame)?;
        if info.document_seq != sequence {
            return None;
        }
        let grant = runtime
            .state
            .attachment(page)?
            .worlds
            .iter()
            .find(|world| {
                world.install_id == request.install_id
                    && world.name == request.world
                    && world.matches.iter().any(|rule| rule.matches(info))
            })?;
        let fallback = if grant.bootstrap.is_empty() {
            runtime
                .pages
                .get(&page)?
                .frames
                .get(&frame)?
                .scripts
                .as_ref()
                .and_then(|table| {
                    table
                        .entries
                        .iter()
                        .find(|entry| {
                            entry.install_id == request.install_id
                                && entry.world == request.world
                                && entry.rule.matches(info)
                                && !entry.bootstrap.is_empty()
                        })
                        .map(|entry| (table.generation, entry.clone()))
                })
        } else {
            None
        };
        Some((grant.bootstrap.clone(), fallback))
    })
    .flatten();
    let Some((bootstrap, fallback)) = bootstrap else {
        return false;
    };
    if !install_bridge(
        Binding {
            page,
            frame,
            document_seq: sequence,
            world: request.world.clone(),
            external_install: None,
        },
        &request.install_id,
        &bootstrap,
    ) {
        return false;
    }
    let Some((table_generation, entry)) = fallback else {
        return true;
    };
    if let Some(outcome) = with_runtime(|runtime| {
        runtime
            .pages
            .get(&page)?
            .frames
            .get(&frame)?
            .bootstrapped
            .get(&request.world)
            .copied()
    })
    .flatten()
    {
        return outcome;
    }
    let Some(context) = context(page, frame, &request.world) else {
        return false;
    };
    let charged = with_runtime(|runtime| {
        let charged = runtime
            .state
            .charge_script_bytes(page, frame, sequence, entry.bootstrap.len())
            .is_ok();
        if let Some(frame) = runtime
            .pages
            .get_mut(&page)
            .and_then(|page| page.frames.get_mut(&frame))
        {
            frame.bootstrapped.insert(request.world.clone(), false);
        }
        charged
    })
    .unwrap_or(false);
    let success = charged
        && evaluate(
            &context,
            &entry.bootstrap,
            "zephium://native/content-bootstrap",
        )
        .1;
    with_runtime(|runtime| {
        if let Some(frame) = runtime
            .pages
            .get_mut(&page)
            .and_then(|page| page.frames.get_mut(&frame))
        {
            frame.bootstrapped.insert(request.world.clone(), success);
        }
        runtime.state.queue_frame_event(
            page,
            FrameEvent::Evaluated(Evaluation {
                install_id: entry.install_id,
                script_id: entry.script_id,
                world: entry.world,
                frame_id: frame,
                document_seq: sequence,
                table_generation,
                file_index: u16::MAX,
                success,
            }),
        );
    });
    flush_frame_events(page) && success
}

pub(super) fn inject(page: u64, request: InjectRequest) -> bool {
    if !with_runtime(|runtime| {
        runtime
            .state
            .validate_message(page, &request.token, request.generation)
    })
    .unwrap_or(false)
    {
        return false;
    }
    let targets = with_runtime(|runtime| match request.target {
        InjectionTarget::Frame {
            frame_id,
            document_seq,
        } => vec![(frame_id, document_seq)],
        InjectionTarget::AllFrames => runtime
            .pages
            .get(&page)
            .map(|native| {
                native
                    .frames
                    .keys()
                    .filter_map(|frame| {
                        runtime
                            .state
                            .frame(page, *frame)
                            .map(|info| (*frame, info.document_seq))
                    })
                    .collect()
            })
            .unwrap_or_default(),
    })
    .unwrap_or_default();
    if targets.is_empty() {
        publish_result(
            PendingInjection {
                page,
                generation: request.generation,
                request_id: request.request_id,
                frame_id: 0,
                document_seq: 0,
                world: request.world.clone(),
                timeout_source: 0,
            },
            InjectionStatus::StaleTarget,
            "null".into(),
        );
    }
    for (frame_id, sequence) in targets {
        let pending = PendingInjection {
            page,
            generation: request.generation,
            request_id: request.request_id,
            frame_id,
            document_seq: sequence,
            world: request.world.clone(),
            timeout_source: 0,
        };
        let admission = with_runtime(|runtime| {
            let info = runtime
                .state
                .frame(page, frame_id)
                .ok_or(InjectionStatus::StaleTarget)?;
            if info.document_seq != sequence {
                return Err(InjectionStatus::StaleTarget);
            }
            let attach = runtime
                .state
                .attachment(page)
                .ok_or(InjectionStatus::StaleTarget)?;
            let grant = attach.worlds.iter().any(|world| {
                world.install_id == request.install_id
                    && (request.world.is_empty() || request.world == world.name)
                    && world.matches.iter().any(|rule| rule.matches(info))
            });
            if !grant {
                return Err(InjectionStatus::Denied);
            }
            if runtime.injections.len() >= MAX_PENDING_INJECTIONS {
                return Err(InjectionStatus::Denied);
            }
            runtime
                .state
                .charge_script_bytes(
                    page,
                    frame_id,
                    sequence,
                    request.source.len().saturating_add(request.args_json.len()),
                )
                .map_err(|_| InjectionStatus::Oversized)?;
            Ok(())
        })
        .unwrap_or(Err(InjectionStatus::StaleTarget));
        if let Err(status) = admission {
            publish_result(pending, status, "null".into());
            continue;
        }
        if matches!(
            request.kind,
            InjectionKind::InsertCss | InjectionKind::RemoveCss
        ) {
            publish_result(pending, InjectionStatus::Unsupported, "null".into());
            continue;
        }
        if !prepare_injection_world(page, frame_id, sequence, &request) {
            publish_result(pending, InjectionStatus::EvaluationFailed, "null".into());
            continue;
        }
        if !with_runtime(|runtime| {
            runtime
                .state
                .validate_message(page, &request.token, request.generation)
                && runtime
                    .state
                    .frame(page, frame_id)
                    .is_some_and(|frame| frame.document_seq == sequence)
        })
        .unwrap_or(false)
        {
            publish_result(pending, InjectionStatus::StaleTarget, "null".into());
            continue;
        }
        let Some(target_context) = context(page, frame_id, &request.world) else {
            publish_result(pending, InjectionStatus::StaleTarget, "null".into());
            continue;
        };
        let Some(private_context) = context(page, frame_id, FACT_WORLD) else {
            publish_result(pending, InjectionStatus::StaleTarget, "null".into());
            continue;
        };
        let (awaiter, success) = evaluate(
            &private_context,
            AWAIT_RESULT,
            "zephium://native/injection-result",
        );
        let Some(awaiter) = awaiter.filter(|_| success) else {
            publish_result(pending, InjectionStatus::EvaluationFailed, "null".into());
            continue;
        };
        let id = with_runtime(|runtime| {
            runtime.next_injection = runtime.next_injection.checked_add(1)?;
            let id = runtime.next_injection;
            runtime.injections.insert(id, pending);
            Some(id)
        })
        .flatten();
        let Some(id) = id else {
            continue;
        };
        unsafe {
            let mut value_type = raw::jsc_value_get_type();
            let callback_data = Box::into_raw(Box::new(id));
            let Some(done) = Owned::full(raw::jsc_value_new_functionv(
                private_context.0,
                c"injectionComplete".as_ptr(),
                callback(injection_complete as *const ()),
                callback_data.cast(),
                Some(injection_id_destroyed),
                G_TYPE_BOOLEAN,
                1,
                &mut value_type,
            )) else {
                drop(Box::from_raw(callback_data));
                finish(id, InjectionStatus::EvaluationFailed, "null".into());
                continue;
            };
            let timeout_source = raw::g_timeout_add_full(
                0,
                request.timeout_ms,
                Some(injection_timeout),
                Box::into_raw(Box::new(id)).cast(),
                Some(injection_id_destroyed),
            );
            with_runtime(|runtime| {
                if let Some(pending) = runtime.injections.get_mut(&id) {
                    pending.timeout_source = timeout_source;
                }
            });
            let value = if request.kind == InjectionKind::Function {
                let Ok(args_json) = CString::new(request.args_json.as_str()) else {
                    finish(id, InjectionStatus::InvalidArguments, "null".into());
                    continue;
                };
                let Some(args) = Owned::full(raw::jsc_value_new_from_json(
                    target_context.0,
                    args_json.as_ptr(),
                )) else {
                    finish(id, InjectionStatus::InvalidArguments, "null".into());
                    continue;
                };
                if raw::jsc_value_is_array(args.0) == 0 {
                    finish(id, InjectionStatus::InvalidArguments, "null".into());
                    continue;
                }
                let (function, success) = evaluate(
                    &target_context,
                    &format!("({}\n)", request.source),
                    &request.source_url,
                );
                let Some(function) = function
                    .filter(|function| success && raw::jsc_value_is_function(function.0) != 0)
                else {
                    finish(id, InjectionStatus::EvaluationFailed, "null".into());
                    continue;
                };
                let (invoke, success) = evaluate(
                    &private_context,
                    "((functionValue, args) => Reflect.apply(functionValue, undefined, args))",
                    "zephium://native/invoke",
                );
                let Some(invoke) = invoke.filter(|_| success) else {
                    finish(id, InjectionStatus::EvaluationFailed, "null".into());
                    continue;
                };
                let mut arguments = [function.0, args.0];
                let value = Owned::full(raw::jsc_value_function_callv(
                    invoke.0,
                    2,
                    arguments.as_mut_ptr(),
                ));
                if !raw::jsc_context_get_exception(private_context.0).is_null() {
                    raw::jsc_context_clear_exception(private_context.0);
                    finish(id, InjectionStatus::EvaluationFailed, "null".into());
                    continue;
                }
                value
            } else {
                let (value, success) =
                    evaluate(&target_context, &request.source, &request.source_url);
                if !success {
                    finish(id, InjectionStatus::EvaluationFailed, "null".into());
                    continue;
                }
                value
            };
            let Some(value) = value else {
                finish(id, InjectionStatus::EvaluationFailed, "null".into());
                continue;
            };
            let mut arguments = [value.0, done.0];
            drop(Owned::full(raw::jsc_value_function_callv(
                awaiter.0,
                2,
                arguments.as_mut_ptr(),
            )));
            if !raw::jsc_context_get_exception(private_context.0).is_null() {
                raw::jsc_context_clear_exception(private_context.0);
                finish(id, InjectionStatus::EvaluationFailed, "null".into());
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(json: String) -> InjectionResult {
        InjectionResult {
            generation: 1,
            request_id: 1,
            frame_id: 1,
            document_seq: 1,
            world: String::new(),
            status: InjectionStatus::Success,
            result_json: json,
        }
    }

    #[test]
    fn unserializable_result_settles_with_a_bounded_error() {
        for json in [
            "invalid".to_owned(),
            format!("{}0{}", "[".repeat(256), "]".repeat(256)),
        ] {
            let result = validated_result(result(json));
            assert_eq!(result.status, InjectionStatus::EvaluationFailed);
            assert_eq!(result.result_json, "null");
            assert!(protocol::encode_event(&Event::Injection(result)).is_ok());
        }
        let result = validated_result(result(r#"{"answer":42}"#.into()));
        assert_eq!(result.status, InjectionStatus::Success);
        assert_eq!(result.result_json, r#"{"answer":42}"#);
    }
}
