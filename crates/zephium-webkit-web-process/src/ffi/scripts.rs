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
    #[cfg(feature = "native-tests")]
    super::diagnostics::evaluation_begin(&url);
    #[cfg(feature = "native-tests")]
    let started = std::time::Instant::now();
    unsafe {
        let value = Owned::full(raw::jsc_context_evaluate_with_source_uri(
            context.0,
            source.as_ptr().cast(),
            source.len() as isize,
            url.as_ptr(),
            1,
        ));
        let success = raw::jsc_context_get_exception(context.0).is_null();
        #[cfg(feature = "native-tests")]
        super::diagnostics::evaluation(
            context,
            &url,
            success,
            Some((started.elapsed().as_nanos(), source.len())),
        );
        raw::jsc_context_clear_exception(context.0);
        (value, success)
    }
}

fn content_catalog(page: u64, frame: u64, install: &str) -> Option<Arc<str>> {
    with_runtime(|runtime| {
        runtime
            .pages
            .get(&page)?
            .frames
            .get(&frame)?
            .scripts
            .as_ref()?
            .table()
            .catalogs
            .get(install)
            .cloned()
    })
    .flatten()
}

fn evaluate_content_bootstrap(context: &Owned, source: &str, catalog: Option<&str>) -> bool {
    let (function, success) = evaluate(context, source, "zephium://native/content-bootstrap");
    let Some(catalog) = catalog else {
        // A deferred bootstrap requires its separately validated catalog.
        // Never report an uninvoked function as an initialized binding.
        return success
            && !function.is_some_and(|value| unsafe { raw::jsc_value_is_function(value.0) != 0 });
    };
    let Some(function) =
        function.filter(|value| unsafe { raw::jsc_value_is_function(value.0) != 0 })
    else {
        return false;
    };
    if !success {
        return false;
    }
    let Ok(catalog) = CString::new(catalog) else {
        return false;
    };
    unsafe {
        let Some(value) = Owned::full(raw::jsc_value_new_from_json(context.0, catalog.as_ptr()))
        else {
            raw::jsc_context_clear_exception(context.0);
            return false;
        };
        let mut argument = value.0;
        drop(Owned::full(raw::jsc_value_function_callv(
            function.0,
            1,
            &mut argument,
        )));
        let success = raw::jsc_context_get_exception(context.0).is_null();
        #[cfg(feature = "native-tests")]
        super::diagnostics::evaluation(
            context,
            c"zephium://native/content-bootstrap",
            success,
            None,
        );
        raw::jsc_context_clear_exception(context.0);
        success
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

fn entry_admitted(document: &Document, entry: &protocol::ScriptEntry<&str>) -> bool {
    with_runtime(|runtime| {
        let Some(info) = runtime.state.frame(document.page, document.frame) else {
            return false;
        };
        let epoch = runtime
            .pages
            .get(&document.page)
            .and_then(|page| page.frames.get(&document.frame))
            .and_then(|frame| frame.scripts.as_ref())
            .and_then(|table| table.table().admissions.get(&entry.install_id));
        info.document_seq == document.sequence
            && entry.rule.matches(info)
            && runtime
                .state
                .attachment(document.page)
                .is_some_and(|attach| {
                    attach.worlds.iter().any(|world| {
                        world.install_id == entry.install_id
                            && epoch == Some(&world.install_generation)
                            && world.name == entry.admission_world
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
            G_TYPE_INT,
        );
        install_function(
            &context,
            protocol::REGISTER_FUNCTION,
            register_receiver as *const (),
            binding.clone(),
            G_TYPE_BOOLEAN,
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
                    request_id: None,
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
        if native.phases[index] || (phase == RunAt::DocumentIdle && native.idle_scheduled) {
            return None;
        }
        native.phases[index] = true;
        let table = native.scripts.clone()?;
        if phase == RunAt::DocumentIdle {
            native.phases[index] = false;
            native.idle_scheduled = true;
        }
        let attach = runtime.state.attachment(page)?;
        let entries = table
            .table()
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| {
                entry.run_at == phase
                    && entry.rule.matches(&info)
                    && attach.worlds.iter().any(|world| {
                        world.install_id == entry.install_id
                            && world.name == entry.admission_world
                            && world.matches.iter().any(|rule| rule.matches(&info))
                    })
            })
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        Some((table, entries, attach.generation))
    })
    .flatten();
    let Some((table, entries, generation)) = work else {
        return;
    };
    if phase == RunAt::DocumentIdle && entries.is_empty() {
        with_runtime(|runtime| {
            if let Some(frame) = runtime
                .pages
                .get_mut(&page)
                .and_then(|page| page.frames.get_mut(&frame))
            {
                frame.phases[2] = true;
            }
        });
        return;
    }
    if phase == RunAt::DocumentIdle {
        unsafe {
            raw::g_timeout_add_full(
                200,
                1,
                Some(idle_script_file),
                Box::into_raw(Box::new(IdleScripts {
                    document,
                    generation,
                    table: Rc::downgrade(&table),
                    entries: entries.into(),
                    file: 0,
                }))
                .cast(),
                Some(idle_scripts_destroyed),
            );
        }
    } else {
        run_entries(&document, &table, &entries, None);
    }
}

struct IdleScripts {
    document: Document,
    generation: u64,
    table: RcWeak<PublishedScripts>,
    entries: VecDeque<usize>,
    file: usize,
}

unsafe extern "C" fn idle_scripts_destroyed(data: *mut c_void) {
    boundary(|| unsafe {
        drop(Box::from_raw(data.cast::<IdleScripts>()));
    });
}

unsafe extern "C" fn idle_script_file(data: *mut c_void) -> c_int {
    boundary(|| unsafe {
        let waiting = &mut *data.cast::<IdleScripts>();
        let document = &waiting.document;
        let attached = with_runtime(|runtime| {
            runtime
                .state
                .attachment(document.page)
                .is_some_and(|attach| attach.generation == waiting.generation)
        })
        .unwrap_or(false);
        if !attached || !live(document) {
            return 0;
        }
        let Some(table) = waiting.table.upgrade() else {
            return 0;
        };
        if let Some(index) = waiting.entries.front().copied() {
            run_entries(document, &table, &[index], Some(waiting.file));
            waiting.file += 1;
            if waiting.file >= table.table().entries[index].files.len() {
                waiting.entries.pop_front();
                waiting.file = 0;
            }
        }
        if waiting.entries.is_empty() {
            with_runtime(|runtime| {
                if runtime
                    .state
                    .frame(document.page, document.frame)
                    .is_some_and(|frame| frame.document_seq == document.sequence)
                {
                    if let Some(frame) = runtime
                        .pages
                        .get_mut(&document.page)
                        .and_then(|page| page.frames.get_mut(&document.frame))
                    {
                        frame.phases[2] = true;
                    }
                }
            });
            0
        } else {
            1
        }
    })
}

fn run_entries(
    document: &Document,
    table: &PublishedScripts,
    entries: &[usize],
    selected_file: Option<usize>,
) {
    let Document {
        page,
        frame,
        sequence,
    } = *document;
    if !flush_frame_events(page) {
        return;
    }
    let table_generation = table.table().generation;
    'entries: for index in entries.iter().copied() {
        let entry = &table.table().entries[index];
        if with_runtime(|runtime| runtime.state.script_budget_exhausted(page, frame, sequence))
            .unwrap_or(true)
        {
            break;
        }
        if !entry_admitted(document, entry) {
            if !live(document) {
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
                let catalog = content_catalog(page, frame, &entry.install_id);
                let bootstrap_bytes =
                    entry.bootstrap.len() + catalog.as_ref().map_or(0, |catalog| catalog.len());
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
                        .charge_script_bytes(page, frame, sequence, bootstrap_bytes)
                        .is_ok()
                })
                .unwrap_or(false);
                let success = charged
                    && evaluate_content_bootstrap(&context, &entry.bootstrap, catalog.as_deref());
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
                            request_id: None,
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
            if selected_file.is_some_and(|selected| selected != file_index) {
                continue;
            }
            if !entry_admitted(document, entry) {
                break;
            }
            let charged = with_runtime(|runtime| {
                runtime
                    .state
                    .charge_script_bytes(page, frame, sequence, file.source.len())
                    .is_ok()
            })
            .unwrap_or(false);
            #[cfg(feature = "native-tests")]
            super::diagnostics::scope(page, frame, sequence, &entry.world, "manifest");
            let success = charged && evaluate(&context, file.source, &file.url).1;
            with_runtime(|runtime| {
                runtime.state.queue_frame_event(
                    page,
                    FrameEvent::Evaluated(Evaluation {
                        request_id: None,
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

struct StartDocument {
    document: Document,
    generation: u64,
}

/// Schedule in the parser's document-start hook, after its root exists. The
/// callback carries native identity only; JavaScript cannot choose a target.
pub(super) fn install_document_start(page: u64, frame: u64, sequence: u64, generation: u64) {
    let document = Document {
        page,
        frame,
        sequence,
    };
    let entries = with_runtime(|runtime| {
        runtime
            .pages
            .get(&page)?
            .frames
            .get(&frame)?
            .scripts
            .clone()
    })
    .flatten();
    if !entries.is_some_and(|table| {
        table
            .table()
            .entries
            .iter()
            .any(|entry| entry.run_at == RunAt::DocumentStart && entry_admitted(&document, entry))
    }) {
        return;
    }
    let Some(context) = context(page, frame, FACT_WORLD) else {
        return;
    };
    // Initial empty documents can acquire their JavaScript world after their
    // root is complete, without passing through WebKit's UserScript hook.
    let initial_blank = with_runtime(|runtime| {
        runtime
            .state
            .frame(page, frame)
            .is_some_and(|info| info.uri == "about:blank")
    })
    .unwrap_or(false);
    if initial_blank {
        let (root, success) = evaluate(
            &context,
            "document.documentElement !== null",
            "zephium://native/document-start-root",
        );
        if success && root.is_some_and(|root| unsafe { raw::jsc_value_to_boolean(root.0) != 0 }) {
            run_scripts(page, frame, sequence, RunAt::DocumentStart);
            return;
        }
    }
    let name = CString::new(protocol::DOCUMENT_START_FUNCTION).expect("static callback name");
    unsafe {
        let data = Box::into_raw(Box::new(StartDocument {
            document,
            generation,
        }));
        let Some(value) = Owned::full(raw::jsc_value_new_functionv(
            context.0,
            name.as_ptr(),
            callback(document_start as *const ()),
            data.cast(),
            Some(start_document_destroyed),
            G_TYPE_BOOLEAN,
            0,
            ptr::null_mut(),
        )) else {
            drop(Box::from_raw(data));
            return;
        };
        raw::jsc_context_set_value(context.0, name.as_ptr(), value.0);
    }
}

unsafe extern "C" fn start_document_destroyed(data: *mut c_void) {
    boundary(|| unsafe {
        drop(Box::from_raw(data.cast::<StartDocument>()));
    });
}

unsafe extern "C" fn document_start(data: *mut c_void) -> c_int {
    boundary(|| unsafe {
        let start = &*data.cast::<StartDocument>();
        let document = start.document.clone();
        let generation = start.generation;
        if !live(&document)
            || !with_runtime(|runtime| {
                runtime
                    .state
                    .attachment(document.page)
                    .is_some_and(|attach| attach.generation == generation)
            })
            .unwrap_or(false)
        {
            return 0;
        }
        run_scripts(
            document.page,
            document.frame,
            document.sequence,
            RunAt::DocumentStart,
        );
        1
    })
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
        let phase = raw::jsc_value_to_int32(value);
        let event = match phase {
            1 => Some(FrameEvent::ContentLoaded {
                frame_id: document.frame,
                document_seq: document.sequence,
            }),
            2 => Some(FrameEvent::Completed {
                frame_id: document.frame,
                document_seq: document.sequence,
            }),
            _ => None,
        };
        if let Some(event) = event {
            with_runtime(|runtime| runtime.state.queue_frame_event(document.page, event));
            flush_frame_events(document.page);
        }
        match phase {
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
        if value.is_null() || raw::jsc_value_is_string(value) == 0 {
            finish(id, InjectionStatus::EvaluationFailed, "null".into());
            return 0;
        }
        let Some((page, frame)) = with_runtime(|runtime| {
            runtime
                .injections
                .get(&id)
                .map(|pending| (pending.page, pending.frame_id))
        })
        .flatten() else {
            return 0;
        };
        let Some(facts) = context(page, frame, FACT_WORLD) else {
            finish(id, InjectionStatus::StaleTarget, "null".into());
            return 0;
        };
        // Only an immutable primitive enters this private helper, never a callback or thenable.
        let (length, success) = evaluate(
            &facts,
            "(value=>value.length)",
            "zephium://native/result-length",
        );
        let Some(length) = length.filter(|_| success) else {
            finish(id, InjectionStatus::EvaluationFailed, "null".into());
            return 0;
        };
        let mut arguments = [value];
        let Some(size) = Owned::full(raw::jsc_value_function_callv(
            length.0,
            1,
            arguments.as_mut_ptr(),
        )) else {
            finish(id, InjectionStatus::EvaluationFailed, "null".into());
            return 0;
        };
        let size = raw::jsc_value_to_int32(size.0);
        if size <= 0 || size as usize > MAX_RESULT_BYTES + 1 {
            finish(id, InjectionStatus::Oversized, "null".into());
            return 0;
        }
        let pointer = raw::jsc_value_to_string(value);
        if pointer.is_null() {
            finish(id, InjectionStatus::EvaluationFailed, "null".into());
            return 0;
        }
        let bytes = std::ffi::CStr::from_ptr(pointer).to_bytes();
        let text = (bytes.len() <= MAX_RESULT_BYTES + 1)
            .then(|| std::str::from_utf8(bytes).ok().map(str::to_owned))
            .flatten();
        raw::g_free(pointer.cast());
        let Some(text) = text else {
            finish(id, InjectionStatus::Oversized, "null".into());
            return 0;
        };
        if text.encode_utf16().count() != size as usize {
            finish(id, InjectionStatus::EvaluationFailed, "null".into());
            return 0;
        }
        let status = match text.as_bytes()[0] {
            b'0' => InjectionStatus::Success,
            b'1' => InjectionStatus::EvaluationFailed,
            b'2' => InjectionStatus::Oversized,
            _ => InjectionStatus::EvaluationFailed,
        };
        let result = text.get(1..).unwrap_or("null").to_owned();
        finish(id, status, result);
        1
    })
}

// Promise assimilation must never give untrusted thenables a function from the
// private facts world. Capture primitives in the target realm; native checks all output.
const AWAIT_RESULT: &str = r#"(() => {
    'use strict';
    const resolve = Promise.resolve.bind(Promise), stringify = JSON.stringify;
    const then = Function.call.bind(Promise.prototype.then);
    return (value, done, batch) => {
        const fail = code => done(code + 'null');
        then(resolve(value), result => {
            try {
                if (batch) result = result.value;
                let json = stringify(result);
                if (json === undefined) json = 'null';
                if (json.length > 1048576) { fail(2); return; }
                done('0' + json);
            } catch (_) { fail(1); }
        }, () => fail(1));
    };
})()"#;

const BATCH_RESULT: &str = r#"(() => {
    'use strict';
    const P = Promise, resolve = P.resolve.bind(P), then = Function.call.bind(P.prototype.then);
    return (...values) => new P(done => {
        let remaining = values.length, last;
        const settle = (index, value) => {
            if (index === values.length - 1) last = value;
            if (--remaining === 0) done({__proto__: null, value: last});
        };
        for (let i = 0; i < values.length; i++) {
            then(resolve(values[i]), value => settle(i, value), () => settle(i, null));
        }
    });
})()"#;

fn injection_document_live(
    runtime: &Runtime,
    page: u64,
    frame: u64,
    sequence: u64,
    request: &InjectRequest,
) -> bool {
    runtime
        .state
        .validate_message(page, &request.token, request.generation)
        && runtime.state.frame(page, frame).is_some_and(|info| {
            info.document_seq == sequence
                && runtime.state.attachment(page).is_some_and(|attachment| {
                    attachment.worlds.iter().any(|grant| {
                        grant.install_id == request.install_id
                            && grant.name == request.admission_world
                            && (request.world.is_empty()
                                || request.world == request.admission_world)
                            && grant.matches.iter().any(|rule| rule.matches(info))
                    })
                })
        })
}

fn prepare_injection_world(page: u64, frame: u64, sequence: u64, request: &InjectRequest) -> bool {
    if request.world.is_empty() {
        return true;
    }
    let bootstrap = with_runtime(|runtime| {
        let info = runtime.state.frame(page, frame)?;
        if !injection_document_live(runtime, page, frame, sequence, request) {
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
            let current = runtime
                .pages
                .get(&page)?
                .scripts
                .as_ref()
                .filter(|table| {
                    table.table().admissions.get(&request.install_id)
                        == Some(&grant.install_generation)
                        && table
                            .table()
                            .bootstraps
                            .get(&request.world)
                            .is_some_and(|bootstrap| bootstrap.install_id == request.install_id)
                })
                .map(|table| (table.clone(), usize::MAX));
            current.or_else(|| {
                runtime
                    .pages
                    .get(&page)?
                    .frames
                    .get(&frame)?
                    .scripts
                    .as_ref()
                    .filter(|table| {
                        table.table().admissions.get(&request.install_id)
                            == Some(&grant.install_generation)
                    })
                    .and_then(|table| {
                        table
                            .table()
                            .entries
                            .iter()
                            .enumerate()
                            .find(|(_, entry)| {
                                entry.install_id == request.install_id
                                    && entry.world == request.world
                                    && entry.rule.matches(info)
                                    && !entry.bootstrap.is_empty()
                            })
                            .map(|(index, _)| (table.clone(), index))
                    })
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
    if request.world.starts_with("zephium-webext-us-") {
        let cached = with_runtime(|runtime| {
            if !injection_document_live(runtime, page, frame, sequence, request) {
                return None;
            }
            runtime
                .pages
                .get(&page)?
                .frames
                .get(&frame)?
                .bootstrapped
                .get(&request.world)
                .copied()
        })
        .flatten();
        if let Some(outcome) = cached {
            return outcome;
        }
    }
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
    let Some((table, index)) = fallback else {
        return !bootstrap.is_empty();
    };
    let table_generation = table.table().generation;
    let (source, script_id) = if index == usize::MAX {
        (table.table().bootstraps[&request.world].source, "")
    } else {
        let entry = &table.table().entries[index];
        (entry.bootstrap.as_str(), entry.script_id.as_str())
    };
    if let Some(outcome) = with_runtime(|runtime| {
        if !injection_document_live(runtime, page, frame, sequence, request) {
            return None;
        }
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
        return publish_bootstrap_ready(
            page,
            frame,
            sequence,
            request,
            table_generation,
            script_id,
            outcome,
        );
    }
    let Some(context) = context(page, frame, &request.world) else {
        return false;
    };
    let catalog = table.table().catalogs.get(&request.install_id).cloned();
    let bootstrap_bytes = source.len() + catalog.as_ref().map_or(0, |catalog| catalog.len());
    let charged = with_runtime(|runtime| {
        if !injection_document_live(runtime, page, frame, sequence, request) {
            return false;
        }
        let charged = runtime
            .state
            .charge_script_bytes(page, frame, sequence, bootstrap_bytes)
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
    let success = charged && evaluate_content_bootstrap(&context, source, catalog.as_deref());
    with_runtime(|runtime| {
        if !injection_document_live(runtime, page, frame, sequence, request) {
            return;
        }
        if let Some(frame) = runtime
            .pages
            .get_mut(&page)
            .and_then(|page| page.frames.get_mut(&frame))
        {
            frame.bootstrapped.insert(request.world.clone(), success);
        }
    });
    publish_bootstrap_ready(
        page,
        frame,
        sequence,
        request,
        table_generation,
        script_id,
        success,
    )
}

fn publish_bootstrap_ready(
    page: u64,
    frame: u64,
    sequence: u64,
    request: &InjectRequest,
    table_generation: u64,
    script_id: &str,
    success: bool,
) -> bool {
    let current = || {
        with_runtime(|runtime| injection_document_live(runtime, page, frame, sequence, request))
            .unwrap_or(false)
    };
    let published = current()
        && flush_frame_events(page)
        && current()
        && send_event_immediately(
            page,
            Event::Frames {
                generation: request.generation,
                events: vec![FrameEvent::Evaluated(Evaluation {
                    request_id: if script_id.is_empty() {
                        Some(request.request_id)
                    } else {
                        None
                    },
                    install_id: request.install_id.clone(),
                    script_id: script_id.to_owned(),
                    world: request.world.clone(),
                    frame_id: frame,
                    document_seq: sequence,
                    table_generation,
                    file_index: u16::MAX,
                    success,
                })],
            },
        );
    if !published {
        with_runtime(|runtime| {
            if !injection_document_live(runtime, page, frame, sequence, request) {
                return;
            }
            if let Some(frame) = runtime
                .pages
                .get_mut(&page)
                .and_then(|page| page.frames.get_mut(&frame))
            {
                frame.bootstrapped.insert(request.world.clone(), false);
            }
        });
    }
    published && success && current()
}

struct IdleInjection {
    reserved: bool,
    page: u64,
    request: InjectRequest,
    source: Option<RcWeak<PublishedSource>>,
    deadline: std::time::Instant,
}

unsafe extern "C" fn idle_injection_destroyed(data: *mut c_void) {
    boundary(|| unsafe {
        let waiting = Box::from_raw(data.cast::<IdleInjection>());
        if waiting.reserved {
            with_runtime(|runtime| {
                runtime.idle_injections = runtime.idle_injections.saturating_sub(1)
            });
        }
    });
}

unsafe extern "C" fn idle_injection(data: *mut c_void) -> c_int {
    boundary(|| unsafe {
        let waiting = &mut *data.cast::<IdleInjection>();
        let InjectionTarget::Frame {
            frame_id,
            document_seq,
        } = waiting.request.target
        else {
            return 0;
        };
        let state = with_runtime(|runtime| {
            if !runtime.state.validate_message(
                waiting.page,
                &waiting.request.token,
                waiting.request.generation,
            ) || runtime
                .state
                .frame(waiting.page, frame_id)
                .is_none_or(|frame| frame.document_seq != document_seq)
            {
                return None;
            }
            runtime
                .pages
                .get(&waiting.page)?
                .frames
                .get(&frame_id)
                .map(|frame| frame.phases[2])
        })
        .flatten();
        let now = std::time::Instant::now();
        if state == Some(false) && now < waiting.deadline {
            return 1;
        }
        if state == Some(true) && now < waiting.deadline {
            let mut request = waiting.request.clone();
            request.kind = match request.kind {
                InjectionKind::FunctionAtIdle => InjectionKind::Function,
                InjectionKind::ScriptBatchAtIdle => InjectionKind::ScriptBatch,
                _ => InjectionKind::Script,
            };
            request.timeout_ms =
                u32::try_from(waiting.deadline.saturating_duration_since(now).as_millis())
                    .unwrap_or(1)
                    .max(1);
            waiting.reserved = false;
            with_runtime(|runtime| {
                runtime.idle_injections = runtime.idle_injections.saturating_sub(1)
            });
            inject(
                waiting.page,
                request,
                waiting.source.as_ref().and_then(RcWeak::upgrade),
            );
        } else {
            publish_result(
                PendingInjection {
                    page: waiting.page,
                    generation: waiting.request.generation,
                    request_id: waiting.request.request_id,
                    frame_id,
                    document_seq,
                    world: waiting.request.world.clone(),
                    timeout_source: 0,
                },
                if state.is_none() {
                    InjectionStatus::StaleTarget
                } else {
                    InjectionStatus::TimedOut
                },
                "null".into(),
            );
        }
        0
    })
}

pub(super) fn inject(
    page: u64,
    request: InjectRequest,
    mapped: Option<Rc<PublishedSource>>,
) -> bool {
    let source = mapped
        .as_ref()
        .map_or(request.source.as_ref(), |source| source.source());
    if request.source_fd.is_some() != mapped.is_some() {
        return false;
    }
    let batch = if matches!(
        request.kind,
        InjectionKind::ScriptBatch | InjectionKind::ScriptBatchAtIdle
    ) {
        match request.batch_sources(Some(source)) {
            Ok(batch) => Some(batch),
            Err(_) => return false,
        }
    } else {
        None
    };
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
                    && request.admission_world == world.name
                    && (request.world.is_empty() || request.world == request.admission_world)
                    && world.matches.iter().any(|rule| rule.matches(info))
            });
            if !grant {
                return Err(InjectionStatus::Denied);
            }
            if runtime.injections.len() + runtime.idle_injections >= MAX_PENDING_INJECTIONS {
                return Err(InjectionStatus::Denied);
            }
            if !matches!(
                request.kind,
                InjectionKind::ScriptAtIdle
                    | InjectionKind::FunctionAtIdle
                    | InjectionKind::ScriptBatchAtIdle
            ) {
                runtime
                    .state
                    .charge_script_bytes(
                        page,
                        frame_id,
                        sequence,
                        source.len().saturating_add(request.args_json.len()),
                    )
                    .map_err(|_| InjectionStatus::Oversized)?;
            }
            Ok(())
        })
        .unwrap_or(Err(InjectionStatus::StaleTarget));
        if let Err(status) = admission {
            publish_result(pending, status, "null".into());
            continue;
        }
        if matches!(
            request.kind,
            InjectionKind::ScriptAtIdle
                | InjectionKind::FunctionAtIdle
                | InjectionKind::ScriptBatchAtIdle
        ) {
            let mut request = request.clone();
            request.target = InjectionTarget::Frame {
                frame_id,
                document_seq: sequence,
            };
            let deadline = std::time::Instant::now()
                + std::time::Duration::from_millis(u64::from(request.timeout_ms));
            with_runtime(|runtime| {
                runtime.idle_injections += 1;
                if let Some(source) = &mapped {
                    if let Some(frame) = runtime
                        .pages
                        .get_mut(&page)
                        .and_then(|page| page.frames.get_mut(&frame_id))
                    {
                        frame
                            .sources
                            .insert(source.mapping.identity, source.clone());
                    }
                }
            });
            unsafe {
                raw::g_timeout_add_full(
                    0,
                    10,
                    Some(idle_injection),
                    Box::into_raw(Box::new(IdleInjection {
                        reserved: true,
                        page,
                        request,
                        source: mapped.as_ref().map(Rc::downgrade),
                        deadline,
                    }))
                    .cast(),
                    Some(idle_injection_destroyed),
                );
            }
            continue;
        }
        if matches!(
            request.kind,
            InjectionKind::InsertCss | InjectionKind::RemoveCss
        ) {
            publish_result(pending, InjectionStatus::Unsupported, "null".into());
            continue;
        }
        if let Some(source) = &mapped {
            with_runtime(|runtime| {
                if let Some(frame) = runtime
                    .pages
                    .get_mut(&page)
                    .and_then(|page| page.frames.get_mut(&frame_id))
                {
                    frame
                        .sources
                        .insert(source.mapping.identity, source.clone());
                }
            });
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
        let (awaiter, success) = evaluate(
            &target_context,
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
                target_context.0,
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
            #[cfg(feature = "native-tests")]
            super::diagnostics::scope(page, frame_id, sequence, &request.world, "executeScript");
            #[cfg(feature = "native-tests")]
            super::diagnostics::request(request.request_id);
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
                let Some(length) = Owned::full(raw::jsc_value_object_get_property(
                    args.0,
                    c"length".as_ptr(),
                )) else {
                    finish(id, InjectionStatus::InvalidArguments, "null".into());
                    continue;
                };
                let count = raw::jsc_value_to_int32(length.0);
                if count < 0 || count as usize > protocol::MAX_ARGS_JSON_BYTES {
                    finish(id, InjectionStatus::InvalidArguments, "null".into());
                    continue;
                }
                let values: Option<Vec<_>> = (0..count as u32)
                    .map(|index| {
                        Owned::full(raw::jsc_value_object_get_property_at_index(args.0, index))
                    })
                    .collect();
                let Some(values) = values else {
                    finish(id, InjectionStatus::InvalidArguments, "null".into());
                    continue;
                };
                let mut arguments: Vec<_> = values.iter().map(|value| value.0).collect();
                let value = Owned::full(raw::jsc_value_function_callv(
                    function.0,
                    arguments.len() as u32,
                    arguments.as_mut_ptr(),
                ));
                if !raw::jsc_context_get_exception(target_context.0).is_null() {
                    raw::jsc_context_clear_exception(target_context.0);
                    finish(id, InjectionStatus::EvaluationFailed, "null".into());
                    continue;
                }
                value
            } else if let Some(batch) = &batch {
                let (aggregate, success) = evaluate(
                    &target_context,
                    BATCH_RESULT,
                    "zephium://native/batch-result",
                );
                let Some(aggregate) = aggregate.filter(|_| success) else {
                    finish(id, InjectionStatus::EvaluationFailed, "null".into());
                    continue;
                };
                let mut values = Vec::with_capacity(batch.len());
                for item in batch {
                    if !with_runtime(|runtime| {
                        injection_document_live(runtime, page, frame_id, sequence, &request)
                    })
                    .unwrap_or(false)
                    {
                        break;
                    }
                    let (value, success) =
                        evaluate(&target_context, &source[item.range.clone()], &item.url);
                    let value = value.filter(|_| success).or_else(|| {
                        evaluate(&target_context, "void 0", "zephium://native/undefined").0
                    });
                    if let Some(value) = value {
                        values.push(value);
                    }
                }
                if values.len() != batch.len() {
                    finish(id, InjectionStatus::StaleTarget, "null".into());
                    continue;
                }
                let mut arguments: Vec<_> = values.iter().map(|value| value.0).collect();
                Owned::full(raw::jsc_value_function_callv(
                    aggregate.0,
                    arguments.len() as u32,
                    arguments.as_mut_ptr(),
                ))
            } else {
                let (value, success) = evaluate(&target_context, source, &request.source_url);
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
            let Some(batch_flag) = Owned::full(raw::jsc_value_new_from_json(
                target_context.0,
                if batch.is_some() {
                    c"true".as_ptr()
                } else {
                    c"false".as_ptr()
                },
            )) else {
                finish(id, InjectionStatus::EvaluationFailed, "null".into());
                continue;
            };
            let mut arguments = [value.0, done.0, batch_flag.0];
            drop(Owned::full(raw::jsc_value_function_callv(
                awaiter.0,
                3,
                arguments.as_mut_ptr(),
            )));
            if !raw::jsc_context_get_exception(target_context.0).is_null() {
                raw::jsc_context_clear_exception(target_context.0);
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
