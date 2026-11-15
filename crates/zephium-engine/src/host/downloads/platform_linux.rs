use super::*;
use gtk::{gio, glib, prelude::*};
use std::os::{fd::AsRawFd, unix::net::UnixDatagram};
use webkit2gtk::{DownloadExt, URIRequestExt, URIResponseExt, WebViewExt};

pub(super) type Native = webkit2gtk::Download;
pub(super) struct Delegate {
    view: glib::WeakRef<webkit2gtk::WebView>,
    signals: Vec<glib::SignalHandlerId>,
}
impl Drop for Delegate {
    fn drop(&mut self) {
        if let Some(view) = self.view.upgrade() {
            for signal in self.signals.drain(..) {
                view.disconnect(signal);
            }
        }
    }
}
pub(super) type Timer = glib::SourceId;
pub(super) type SavePanel = gtk::FileChooserNative;
pub(super) type DirectoryPanel = gtk::FileChooserNative;
pub(super) type DialogLease = ();

#[derive(Clone)]
pub(super) struct Sender {
    queue: mpsc::Sender<Message>,
    wake: Option<Arc<UnixDatagram>>,
}
impl Sender {
    pub(super) fn send(&self, message: Message) -> Result<(), ()> {
        self.queue.send(message).map_err(|_| ())?;
        if let Some(wake) = &self.wake {
            let _ = wake.send(&[1]);
        }
        Ok(())
    }
}
pub(super) fn channel() -> (Sender, mpsc::Receiver<Message>, Option<UnixDatagram>) {
    let (queue, receiver) = mpsc::channel();
    let sockets = UnixDatagram::pair()
        .ok()
        .filter(|(a, b)| a.set_nonblocking(true).is_ok() && b.set_nonblocking(true).is_ok());
    let (wake, read) = match sockets {
        Some((a, b)) => (Some(Arc::new(a)), Some(b)),
        None => (None, None),
    };
    (Sender { queue, wake }, receiver, read)
}
pub(super) fn stop_timer(timer: Timer) {
    timer.remove();
}
pub(super) fn cancel_directory(panel: &DirectoryPanel) {
    panel.hide();
    panel.destroy();
}
pub(super) fn progress(native: &Native) -> Option<(u64, Option<u64>)> {
    Some((
        native.received_data_length(),
        native
            .response()
            .map(|r| r.content_length())
            .filter(|n| *n > 0),
    ))
}
pub(super) fn completion_bytes(native: &Native) -> Result<Option<u64>, DownloadError> {
    Ok(Some(native.received_data_length()))
}
fn portal_open(path: &std::path::Path, directory: bool) -> Result<Option<()>, DownloadError> {
    use std::os::unix::fs::OpenOptionsExt;
    let connection = match gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE) {
        Ok(connection) => connection,
        Err(_) => return Ok(None),
    };
    let owner = connection
        .call_sync(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "NameHasOwner",
            Some(&("org.freedesktop.portal.Desktop",).to_variant()),
            None,
            gio::DBusCallFlags::NO_AUTO_START,
            1000,
            gio::Cancellable::NONE,
        )
        .ok()
        .and_then(|v| v.get::<(bool,)>())
        .is_some_and(|v| v.0);
    if !owner {
        return Ok(None);
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(|_| DownloadError::ChangedFile)?;
    let fds = gio::UnixFDList::new();
    let handle = fds.append(file).map_err(|_| DownloadError::Unavailable)?;
    let options: HashMap<String, glib::Variant> = HashMap::new();
    connection
        .call_with_unix_fd_list_sync(
            Some("org.freedesktop.portal.Desktop"),
            "/org/freedesktop/portal/desktop",
            "org.freedesktop.portal.OpenURI",
            if directory {
                "OpenDirectory"
            } else {
                "OpenFile"
            },
            Some(&("", glib::variant::Handle(handle), options).to_variant()),
            None,
            gio::DBusCallFlags::NO_AUTO_START,
            1000,
            Some(&fds),
            gio::Cancellable::NONE,
        )
        .map_err(|_| DownloadError::Unavailable)?;
    Ok(Some(()))
}
pub(super) fn reveal(path: &std::path::Path) -> Result<(), DownloadError> {
    if portal_open(path, true)?.is_some() {
        return Ok(());
    }
    let parent = path.parent().ok_or(DownloadError::Destination)?;
    gio::AppInfo::launch_default_for_uri(
        &gio::File::for_path(parent).uri(),
        gio::AppLaunchContext::NONE,
    )
    .map_err(|_| DownloadError::Unavailable)
}
pub(super) fn open(path: &std::path::Path) -> Result<(), DownloadError> {
    if super::super::download_files::must_reveal(path)? {
        return reveal(path);
    }
    if portal_open(path, false)?.is_some() {
        return Ok(());
    }
    let (content, _) = gio::content_type_guess(Some(path), &[]);
    let handler =
        gio::AppInfo::default_for_type(&content, false).ok_or(DownloadError::Unavailable)?;
    handler
        .launch(&[gio::File::for_path(path)], gio::AppLaunchContext::NONE)
        .map_err(|_| DownloadError::Unavailable)
}

pub(super) struct Source {
    permit: EventPermit,
    surface_intent: Arc<AtomicBool>,
    navigation: NavigationEpochTracker,
    activity: NavigationActivity,
    view: glib::WeakRef<webkit2gtk::WebView>,
    window: glib::WeakRef<gtk::Window>,
}
impl Source {
    pub(super) fn in_front(&self) -> bool {
        self.window.upgrade().is_some_and(|w| w.is_active())
    }
    pub(super) fn current(&self) -> bool {
        self.permit.active_token().is_some() && self.navigation.matches_activity(self.activity)
    }
    pub(super) fn live(&self) -> bool {
        self.current()
            && self.surface_intent.load(Ordering::Acquire)
            && self.view.upgrade().is_some_and(|v| v.is_mapped())
            && self.window.upgrade().is_some_and(|w| w.is_visible())
    }
}

impl Downloads {
    pub(super) fn install_wake(self: &Rc<Self>, socket: UnixDatagram) {
        let weak = Rc::downgrade(self);
        let source = glib::source::unix_fd_add_local(
            socket.as_raw_fd(),
            glib::IOCondition::IN,
            move |_, _| {
                let Some(manager) = weak.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                let mut bytes = [0; 256];
                while socket.recv(&mut bytes).is_ok() {}
                manager.tick();
                glib::ControlFlow::Continue
            },
        );
        *self.wake_source.borrow_mut() = Some(source);
    }
    pub(super) fn ensure_timer(self: &Rc<Self>) {
        if let Some(timer) = self.timer.borrow_mut().take() {
            timer.remove();
        }
        let deadline = self
            .active
            .borrow()
            .values()
            .filter(|t| {
                !t.record.state.terminal()
                    && matches!(t.record.state, DownloadState::Pending | DownloadState::Held)
            })
            .filter_map(|t| t.deadline.expires)
            .min();
        if let Some(deadline) = deadline {
            let weak = Rc::downgrade(self);
            let source = glib::timeout_add_local_once(
                deadline.saturating_duration_since(Instant::now()) + Duration::from_millis(1),
                move || {
                    if let Some(manager) = weak.upgrade() {
                        manager.timer.borrow_mut().take();
                        manager.tick();
                    }
                },
            );
            *self.timer.borrow_mut() = Some(source);
        }
    }
    pub(super) fn user_pause(
        self: &Rc<Self>,
        _: Partition,
        _: DownloadId,
    ) -> Result<(), DownloadError> {
        Err(DownloadError::Unsupported)
    }
    pub(super) fn resume(
        self: &Rc<Self>,
        _: Partition,
        _: DownloadId,
    ) -> Result<(), DownloadError> {
        Err(DownloadError::Unsupported)
    }

    pub(in crate::host) fn admit(
        self: &Rc<Self>,
        partition: Partition,
        permit: EventPermit,
        surface_intent: Arc<AtomicBool>,
        navigation: NavigationEpochTracker,
        native: &Native,
        ask: Option<SiteAsk>,
    ) -> Option<DownloadId> {
        let reject = || {
            native.cancel();
            None
        };
        if self.stopping.get()
            || self.retired.borrow().contains(&partition.profile())
            || self.active.borrow().len() >= MAX_ACTIVE_DOWNLOADS
            || self.work.get() >= MAX_BACKGROUND_WORK
            || !self.private_cleanup_capacity(partition)
        {
            return reject();
        }
        let view = native.web_view()?;
        let window = view.toplevel()?.downcast::<gtk::Window>().ok()?;
        let activity = navigation.activity_snapshot()?;
        let source = Source {
            permit,
            surface_intent,
            navigation,
            activity,
            view: view.downgrade(),
            window: window.downgrade(),
        };
        if !source.live() || !source.in_front() {
            return reject();
        }
        let uri = native
            .request()
            .and_then(|r| r.uri())
            .filter(|u| u.len() <= 8192);
        let parsed = uri.as_ref().and_then(|s| url::Url::parse(s).ok());
        if !parsed.as_ref().is_some_and(|u| {
            matches!(u.scheme(), "http" | "https" | "blob" | "data")
                && u.username().is_empty()
                && u.password().is_none()
        }) {
            return reject();
        }
        let origin = source
            .navigation
            .committed_snapshot()
            .and_then(|(_, url)| url::Url::parse(&url).ok())
            .and_then(|url| PageOrigin::from_url(&url).ok())
            .or_else(|| parsed.as_ref().and_then(|u| PageOrigin::from_url(u).ok()))?;
        // GTK has no activation metadata: page downloads require the shared explicit confirmation.
        let site = match self.site_admission(partition.profile(), ask, &origin, false) {
            site::SiteAdmission::Refuse => return reject(),
            site::SiteAdmission::Open => None,
            site::SiteAdmission::Hold(hold) => Some(hold),
        };
        let provenance = (!matches!(partition, Partition::Ephemeral(_))).then(|| {
            (
                uri.map(|u| u.to_string()).unwrap_or_default(),
                source
                    .navigation
                    .committed_snapshot()
                    .map(|(_, url)| url)
                    .unwrap_or_default(),
            )
        });
        let id = DownloadId::generate();
        self.active.borrow_mut().insert(
            id,
            Transfer {
                partition,
                record: DownloadRecord {
                    id,
                    session: self.session,
                    revision: 1,
                    created_at: SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map_or(0, |t| t.as_secs() as i64),
                    filename: "download".into(),
                    source: origin.as_str().into(),
                    source_is_context: true,
                    state: DownloadState::Pending,
                    received: 0,
                    total: None,
                    error: None,
                    destination: None,
                    staging: None,
                    staging_identity: None,
                    identity: None,
                    writer: None,
                    writer_released: false,
                    insecure: false,
                },
                native: native.clone(),
                provenance,
                _delegate: Delegate {
                    view: view.downgrade(),
                    signals: vec![],
                },
                source: Some(source),
                destination_reply: None,
                destination: None,
                panel: None,
                picker_preferences: None,
                on_started: None,
                authorized: false,
                cancelling: false,
                persisting_terminal: false,
                kept: false,
                site,
                confirmed: false,
                deadline: DecisionDeadline::new(DecisionPhase::Admission, Instant::now()),
            },
        );
        native.set_allow_overwrite(false);
        let weak = Rc::downgrade(self);
        native.connect_decide_destination(move |download, suggested| {
            if let Some(manager) = weak.upgrade() {
                manager.destination(id, download, suggested);
            } else {
                download.cancel();
            }
            true
        });
        let weak = Rc::downgrade(self);
        native.connect_received_data(move |_, _| {
            if let Some(manager) = weak.upgrade() {
                manager.tick();
            }
        });
        let weak = Rc::downgrade(self);
        native.connect_failed(move |_, _| {
            if let Some(manager) = weak.upgrade() {
                manager.note_native_error(id, DownloadError::Network);
            }
        });
        let weak = Rc::downgrade(self);
        native.connect_finished(move |_| {
            if let Some(manager) = weak.upgrade() {
                let failure = manager
                    .active
                    .borrow()
                    .get(&id)
                    .map(|t| (t.cancelling, t.record.error));
                match failure {
                    Some((true, _)) => manager.terminal(id, DownloadState::Cancelled, None),
                    Some((_, Some(error))) => manager.native_failed_with_error(id, error),
                    Some(_) => manager.native_finished(id),
                    None => {}
                }
            }
        });
        let weak = Rc::downgrade(self);
        let unmap = view.connect_unmap(move |_| {
            if let Some(manager) = weak.upgrade() {
                manager.tick();
            }
        });
        let weak = Rc::downgrade(self);
        let load = view.connect_load_changed(move |_, _| {
            if let Some(manager) = weak.upgrade() {
                manager.tick();
            }
        });
        let weak = Rc::downgrade(self);
        let destroy = view.connect_destroy(move |_| {
            if let Some(manager) = weak.upgrade() {
                manager.tick();
            }
        });
        if let Some(transfer) = self.active.borrow_mut().get_mut(&id) {
            transfer._delegate.signals = vec![unmap, load, destroy];
        }
        self.ensure_recovery(partition);
        self.ensure_timer();
        self.changed(partition.profile());
        Some(id)
    }

    fn destination(self: &Rc<Self>, id: DownloadId, native: &Native, suggested: &str) {
        let partition = {
            let mut active = self.active.borrow_mut();
            let Some(transfer) = active
                .get_mut(&id)
                .filter(|t| !t.cancelling && t.destination_reply.is_none())
            else {
                native.cancel();
                return;
            };
            transfer.record.filename = if suggested.len() <= 4096 {
                safe_filename(suggested)
            } else {
                "download".into()
            };
            transfer.record.total = native
                .response()
                .map(|r| r.content_length())
                .filter(|n| *n > 0);
            transfer.record.insecure =
                is_insecure_response(native.response().and_then(|r| r.uri()).as_deref());
            let download = native.clone();
            transfer.destination_reply = Some(DestinationReply::new(move |path| {
                if let Some(path) = path.and_then(|p| p.to_str().map(str::to_owned)) {
                    download.set_destination(&path);
                } else {
                    download.cancel();
                }
            }));
            transfer.partition
        };
        let cached = self.preferences.borrow().get(&partition.profile()).cloned();
        if let Some(preferences) = cached {
            self.decide(id, preferences);
        } else if matches!(partition, Partition::Ephemeral(_)) {
            self.decide(id, DownloadPreferences::default());
        } else {
            self.work.set(self.work.get() + 1);
            let sender = self.sender.clone();
            if !self.store.download_call(
                partition.profile(),
                DownloadStoreCall::Preferences,
                Box::new(move |reply| {
                    let _ = sender.send(Message::Preferences(id, reply));
                }),
            ) {
                let _ = self.sender.send(Message::Preferences(
                    id,
                    DownloadStoreReply::Error(DownloadError::Storage),
                ));
            }
        }
        self.ensure_timer();
    }

    pub(super) fn choose_destination(
        self: &Rc<Self>,
        id: DownloadId,
        preferences: DownloadPreferences,
    ) {
        let context = self.active.borrow().get(&id).and_then(|t| {
            t.source.as_ref().map(|s| {
                (
                    s.live() && s.in_front(),
                    s.window.upgrade(),
                    t.record.filename.clone(),
                )
            })
        });
        let Some((true, Some(window), filename)) = context else {
            self.cancel(id, None);
            return;
        };
        if !preferences.ask_destination {
            let directory = preferences
                .directory
                .as_ref()
                .map(PathBuf::from)
                .or_else(dirs::download_dir);
            if let Some(directory) = directory {
                self.prepare(id, directory.join(filename), preferences.directory_identity);
            } else {
                self.cancel(id, Some(DownloadError::Destination));
            }
            return;
        }
        let panel = gtk::FileChooserNative::new(
            Some("Save download"),
            Some(&window),
            gtk::FileChooserAction::Save,
            Some("Save"),
            Some("Cancel"),
        );
        panel.set_current_name(&filename);
        panel.set_do_overwrite_confirmation(true);
        let weak = Rc::downgrade(self);
        panel.connect_response(move |panel, response| {
            let path = (response == gtk::ResponseType::Accept)
                .then(|| panel.filename())
                .flatten();
            panel.hide();
            if let Some(manager) = weak.upgrade() {
                if let Some(transfer) = manager.active.borrow_mut().get_mut(&id) {
                    transfer.panel = None;
                }
                match path {
                    Some(path) => manager.prepare(id, path, None),
                    None => manager.cancel(id, None),
                }
            }
        });
        if let Some(transfer) = self.active.borrow_mut().get_mut(&id) {
            transfer.panel = Some(panel.clone());
            transfer.deadline = transfer.deadline.waiting_for_picker(Instant::now());
        }
        panel.show();
        self.ensure_timer();
    }

    pub(super) fn cancel(&self, id: DownloadId, error: Option<DownloadError>) {
        let work = {
            let mut active = self.active.borrow_mut();
            let Some(t) = active.get_mut(&id) else {
                return;
            };
            if t.cancelling
                || t.record.state.terminal()
                || t.record.state == DownloadState::Finalizing
            {
                return;
            }
            if matches!(t.record.state, DownloadState::Held | DownloadState::Paused) {
                drop(active);
                self.terminal(id, DownloadState::Cancelled, error);
                return;
            }
            t.cancelling = true;
            t.record.state = DownloadState::Cancelling;
            t.record.error = error;
            t.record.revision += 1;
            (t.native.clone(), t.destination_reply.take(), t.panel.take())
        };
        if let Some(panel) = work.2 {
            cancel_directory(&panel);
        }
        drop(work.1);
        work.0.cancel();
    }
    pub(super) fn choose_directory(
        self: &Rc<Self>,
        token: u64,
        partition: Partition,
        preferences: DownloadPreferences,
        done: DownloadCompletion,
    ) {
        let window = gtk::Window::list_toplevels()
            .into_iter()
            .filter_map(|w| w.downcast::<gtk::Window>().ok())
            .find(|w| w.is_active());
        let Some(window) = window else {
            done.finish(DownloadResponse::Error {
                error: DownloadError::Unavailable,
            });
            return;
        };
        let panel = gtk::FileChooserNative::new(
            Some("Downloads folder"),
            Some(&window),
            gtk::FileChooserAction::SelectFolder,
            Some("Select"),
            Some("Cancel"),
        );
        self.calls.borrow_mut().insert(
            token,
            UiCall {
                partition,
                call: DownloadCall::ChooseDirectory,
                done,
            },
        );
        self.directory_panels
            .borrow_mut()
            .insert(token, (panel.clone(), ()));
        let weak = Rc::downgrade(self);
        panel.connect_response(move |panel, response| {
            panel.hide();
            let Some(manager) = weak.upgrade() else {
                return;
            };
            manager.directory_panels.borrow_mut().remove(&token);
            let Some(request) = manager.calls.borrow_mut().remove(&token) else {
                return;
            };
            if let Some(path) = (response == gtk::ResponseType::Accept)
                .then(|| panel.filename())
                .flatten()
            {
                let sender = manager.sender.clone();
                let preferences = preferences.clone();
                manager.work.set(manager.work.get() + 1);
                if std::thread::Builder::new()
                    .name("zephium-download-directory".into())
                    .spawn(move || {
                        let result = super::super::download_files::select_directory(path);
                        let _ =
                            sender.send(Message::DirectorySelected(request, preferences, result));
                    })
                    .is_err()
                {
                    manager.work.set(manager.work.get() - 1);
                }
            } else {
                request.done.finish(DownloadResponse::Error {
                    error: DownloadError::Cancelled,
                });
            }
        });
        panel.show();
    }
}

#[cfg(test)]
impl Downloads {
    pub(in crate::host) fn test_directory(&self, profile: ProfileId, directory: PathBuf) {
        let (directory, receipt) =
            super::super::download_files::select_directory(directory).unwrap();
        self.preferences.borrow_mut().insert(
            profile,
            DownloadPreferences {
                directory: Some(directory),
                directory_identity: Some(receipt),
                ask_destination: false,
            },
        );
    }
}

impl Drop for Downloads {
    fn drop(&mut self) {
        if let Some(source) = self.wake_source.get_mut().take() {
            source.remove();
        }
        if let Some(timer) = self.timer.get_mut().take() {
            timer.remove();
        }
    }
}
