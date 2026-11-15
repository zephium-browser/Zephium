//! Profile-owned memory/BFCache invalidation after webRequest handler changes.
use super::*;
use serde_json::Value;
use webkit2gtk::{WebContextExt, WebsiteDataManagerExtManual};
use zephium_webext_runtime::{context::Sender, dispatch::Responder, ApiError};

const MAX_PENDING: usize = 128;

struct Job {
    sender: Box<Sender>,
    responder: Responder,
}

#[derive(Default)]
pub(super) struct Backend {
    pending: RefCell<Vec<Job>>,
    count: Cell<usize>,
    failed: Cell<bool>,
    retired: Cell<bool>,
    task: RefCell<Option<glib::JoinHandle<()>>>,
}

impl Backend {
    pub(super) fn retire(&self) {
        self.retired.set(true);
        if let Some(task) = self.task.borrow_mut().take() {
            task.abort();
        }
        self.pending.borrow_mut().clear();
        self.count.set(0);
    }
}

impl Drop for Backend {
    fn drop(&mut self) {
        self.retire();
    }
}

fn allowed(inner: &Inner, sender: &Sender) -> bool {
    !inner.retired.get()
        && !inner.handler_cache.retired.get()
        && inner
            .bridge
            .with_runtime(|host, _| host.handler_cache_allowed(sender))
            .unwrap_or(false)
}

pub(super) fn submit(inner: &Rc<Inner>, sender: Box<Sender>, responder: Responder) {
    let backend = &inner.handler_cache;
    if !allowed(inner, &sender) || backend.failed.get() || backend.count.get() >= MAX_PENDING {
        let error = if backend.failed.get() {
            ApiError::other("The memory cache clear timed out.")
        } else if backend.count.get() >= MAX_PENDING {
            ApiError::too_many_pending()
        } else {
            ApiError::context_invalidated()
        };
        let _ = inner
            .bridge
            .with_runtime(|host, out| host.settle_call(responder, Err(error), out));
        return;
    }
    backend.count.set(backend.count.get() + 1);
    backend.pending.borrow_mut().push(Job { sender, responder });
    if backend.task.borrow().is_some() {
        return;
    }
    let weak = Rc::downgrade(inner);
    let task = glib::MainContext::default().spawn_local(async move {
        loop {
            let Some(inner) = weak.upgrade().filter(|inner| !inner.retired.get()) else {
                return;
            };
            let jobs = std::mem::take(&mut *inner.handler_cache.pending.borrow_mut());
            let mut admitted = Vec::new();
            for job in jobs {
                if allowed(&inner, &job.sender) {
                    admitted.push(job);
                } else {
                    inner
                        .handler_cache
                        .count
                        .set(inner.handler_cache.count.get().saturating_sub(1));
                    let _ = inner.bridge.with_runtime(|host, out| {
                        host.settle_call(job.responder, Err(ApiError::context_invalidated()), out)
                    });
                }
            }
            if admitted.is_empty() {
                inner.handler_cache.task.borrow_mut().take();
                inner.schedule();
                return;
            }
            let manager = inner.context.website_data_manager();
            drop(inner);
            let (result, timed_out) = if let Some(manager) = manager {
                let clear = gio::GioFuture::new(&manager, |manager, cancel, done| {
                    manager.clear(
                        webkit2gtk::WebsiteDataTypes::MEMORY_CACHE,
                        glib::TimeSpan(0),
                        Some(cancel),
                        move |result| done.resolve(result),
                    );
                });
                futures_lite::future::race(
                    async {
                        (
                            clear.await.map_err(|_| {
                                ApiError::other("The browser could not clear its memory cache.")
                            }),
                            false,
                        )
                    },
                    async {
                        async_io::Timer::after(Duration::from_secs(15)).await;
                        (
                            Err(ApiError::other("The memory cache clear timed out.")),
                            true,
                        )
                    },
                )
                .await
            } else {
                (
                    Err(ApiError::other("This function is not supported.")),
                    false,
                )
            };
            let Some(inner) = weak.upgrade().filter(|inner| !inner.retired.get()) else {
                return;
            };
            if timed_out {
                // GCancellable cancels the GTask waiter, not WebKit's removeData
                // operation. Never start another clear in this profile after a
                // timeout: the engine may still be executing the first one.
                inner.handler_cache.failed.set(true);
                admitted.extend(std::mem::take(
                    &mut *inner.handler_cache.pending.borrow_mut(),
                ));
            }
            for job in admitted {
                inner
                    .handler_cache
                    .count
                    .set(inner.handler_cache.count.get().saturating_sub(1));
                let result = if allowed(&inner, &job.sender) {
                    result.clone().map(|()| Value::Null)
                } else {
                    Err(ApiError::context_invalidated())
                };
                let _ = inner
                    .bridge
                    .with_runtime(|host, out| host.settle_call(job.responder, result, out));
            }
            inner.schedule();
            if inner.handler_cache.pending.borrow().is_empty() {
                inner.handler_cache.task.borrow_mut().take();
                return;
            }
        }
    });
    *backend.task.borrow_mut() = Some(task);
}
