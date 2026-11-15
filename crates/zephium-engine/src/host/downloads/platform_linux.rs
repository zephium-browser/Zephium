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
    let received = native.received_data_length();
    if let Some(response) = native.response() {
        let encoded = response
            .http_headers()
            .and_then(|headers| headers.one("Content-Encoding"))
            .is_some_and(|encoding| !encoding.eq_ignore_ascii_case("identity"));
        // Soup can finish a truncated response without emitting Download::failed.
        // Encoded Content-Length counts compressed bytes, not the published file.
        if !encoded && response.content_length() > 0 && received != response.content_length() {
            return Err(DownloadError::ConnectionLost);
        }
    }
    Ok(Some(received))
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
    let message = gio::DBusMessage::new_method_call(
        Some("org.freedesktop.portal.Desktop"),
        "/org/freedesktop/portal/desktop",
        Some("org.freedesktop.portal.OpenURI"),
        if directory {
            "OpenDirectory"
        } else {
            "OpenFile"
        },
    );
    message.set_flags(gio::DBusMessageFlags::NO_AUTO_START);
    message.set_body(&("", glib::variant::Handle(handle), options).to_variant());
    message.set_unix_fd_list(Some(&fds));
    // A portal reply carries an object path and no FDs. The older typed GIO
    // call wrapper incorrectly requires a non-null output FD list.
    let (reply, _) = connection
        .send_message_with_reply_sync(
            &message,
            gio::DBusSendMessageFlags::NONE,
            1000,
            gio::Cancellable::NONE,
        )
        .map_err(|_| DownloadError::Unavailable)?;
    if reply.message_type() != gio::DBusMessageType::MethodReturn
        || reply
            .body()
            .is_none_or(|body| body.type_().as_str() != "(o)")
    {
        return Err(DownloadError::Unavailable);
    }
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
        if self.stopping.get()
            || self.retired.borrow().contains(&partition.profile())
            || self.active.borrow().len() >= MAX_ACTIVE_DOWNLOADS
            || self.work.get() >= MAX_BACKGROUND_WORK
            || !self.private_cleanup_capacity(partition)
        {
            return None;
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
            return None;
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
            return None;
        }
        let origin = source
            .navigation
            .committed_snapshot()
            .and_then(|(_, url)| url::Url::parse(&url).ok())
            .and_then(|url| PageOrigin::from_url(&url).ok())
            .or_else(|| parsed.as_ref().and_then(|u| PageOrigin::from_url(u).ok()))?;
        // GTK exposes navigation activation but no authenticated action-to-download join.
        let site = match self.site_admission(partition.profile(), ask, &origin, false) {
            site::SiteAdmission::Refuse => return None,
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
                    source_is_context: parsed
                        .as_ref()
                        .is_some_and(|url| matches!(url.scheme(), "blob" | "data")),
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
            if let Some(directory) = directory.filter(|path| path.is_dir()) {
                self.prepare(id, directory.join(filename), preferences.directory_identity);
                return;
            }
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
        // Dropping the outstanding destination reply already cancels WebKit.
        // A second cancellation races its asynchronous terminal callback.
        if let Some(reply) = work.1 {
            reply.finish(None);
        } else {
            work.0.cancel();
        }
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
    pub(in crate::host) fn test_missing_destination(&self, profile: ProfileId) {
        let directory = tempfile::tempdir().unwrap();
        self.test_directory(profile, directory.path().to_owned());
    }
    pub(in crate::host) fn test_save_panel(&self) -> Option<gtk::FileChooserNative> {
        self.active
            .borrow()
            .values()
            .find_map(|transfer| transfer.panel.clone())
    }
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

#[cfg(test)]
mod native_tests {
    use super::*;
    use std::io::BufRead;
    struct Service(std::process::Child);
    impl Drop for Service {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[test]
    #[ignore = "isolated Xvfb and private D-Bus; test-owned GIO handlers and portal only"]
    fn native_open_uses_gio_or_portal_and_reveals_executables() {
        gtk::init().unwrap();
        let data = PathBuf::from(std::env::var_os("XDG_DATA_HOME").unwrap());
        let root = tempfile::tempdir().unwrap();
        let plain = root.path().join("safe.txt");
        let script = root.path().join("script.txt");
        std::fs::write(&plain, "fixture text").unwrap();
        std::fs::write(&script, "#!/bin/sh\nexit 0\n").unwrap();
        let record = root.path().join("opened.jsonl");
        let handler = root.path().join("handler.py");
        std::fs::write(&handler,"import sys,json\nwith open(sys.argv[1],'a') as f:f.write(json.dumps(sys.argv[2:])+'\\n')\n").unwrap();
        let apps = data.join("applications");
        std::fs::create_dir_all(&apps).unwrap();
        std::fs::write(apps.join("download-fixture.desktop"),format!("[Desktop Entry]\nType=Application\nName=Download fixture\nExec=/usr/bin/python3 {} {} %f\nMimeType=text/plain;inode/directory;\nNoDisplay=true\n",handler.display(),record.display())).unwrap();
        std::fs::write(apps.join("mimeapps.list"),"[Default Applications]\ntext/plain=download-fixture.desktop;\ninode/directory=download-fixture.desktop;\n").unwrap();
        assert!(portal_open(&plain, false).unwrap().is_none());
        open(&plain).unwrap();
        open(&script).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let lines = loop {
            let text = std::fs::read_to_string(&record).unwrap_or_default();
            if text.lines().count() == 2 {
                break text;
            }
            assert!(Instant::now() < deadline, "GIO handlers did not run");
            std::thread::sleep(Duration::from_millis(10));
        };
        let paths: Vec<Vec<String>> = lines
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert!(paths.contains(&vec![plain.to_str().unwrap().into()]));
        assert!(paths.contains(&vec![root.path().to_str().unwrap().into()]));
        let portal_log = root.path().join("portal.jsonl");
        let mut service=Service(std::process::Command::new("python3").args(["-u","-c",r#"
import dbus,dbus.service,sys,os,json,stat
from dbus.mainloop.glib import DBusGMainLoop
from gi.repository import GLib
DBusGMainLoop(set_as_default=True);bus=dbus.SessionBus();name=dbus.service.BusName('org.freedesktop.portal.Desktop',bus,do_not_queue=True)
class Portal(dbus.service.Object):
 def record(self,kind,fd):
  fd=fd.take();info=os.fstat(fd);assert stat.S_ISREG(info.st_mode);assert info.st_mode&0o111==0
  with open(sys.argv[1],'a') as log:log.write(json.dumps([kind,os.read(fd,100).decode()])+'\n')
  os.close(fd);return dbus.ObjectPath('/org/freedesktop/portal/desktop/request/fixture/1')
 @dbus.service.method('org.freedesktop.portal.OpenURI',in_signature='sha{sv}',out_signature='o')
 def OpenFile(self,parent,fd,options):return self.record('file',fd)
 @dbus.service.method('org.freedesktop.portal.OpenURI',in_signature='sha{sv}',out_signature='o')
 def OpenDirectory(self,parent,fd,options):return self.record('reveal',fd)
p=Portal(bus,'/org/freedesktop/portal/desktop');print('READY',flush=True);GLib.MainLoop().run()
"#]).arg(&portal_log).stdout(std::process::Stdio::piped()).spawn().unwrap());
        let mut line = String::new();
        std::io::BufReader::new(service.0.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        assert_eq!(line.trim(), "READY");
        open(&plain).unwrap();
        open(&script).unwrap();
        let calls = std::fs::read_to_string(&portal_log).unwrap();
        let calls: Vec<(String, String)> = calls
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(
            calls,
            vec![
                ("file".into(), "fixture text".into()),
                ("reveal".into(), "#!/bin/sh\nexit 0\n".into())
            ]
        );
        assert_eq!(std::fs::read_to_string(&record).unwrap().lines().count(), 2);
    }
}
