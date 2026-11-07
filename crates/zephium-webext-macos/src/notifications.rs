//! `chrome.notifications` through the Mac's notification center. Each
//! notification names its extension under the title, buttons become the
//! notification's actions, and clicks, button presses and dismissals come
//! back as the extension's events. They show while Zephium is in front too,
//! as Chrome's do.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::rc::{Rc, Weak as RcWeak};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{Bool, NSObject, ProtocolObject};
use objc2::{define_class, msg_send, AllocAnyThread};
use objc2_foundation::{NSArray, NSBundle, NSError, NSObjectProtocol, NSSet, NSString};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNMutableNotificationContent, UNNotification, UNNotificationAction,
    UNNotificationActionOptions, UNNotificationCategory, UNNotificationCategoryOptions,
    UNNotificationDefaultActionIdentifier, UNNotificationDismissActionIdentifier,
    UNNotificationPresentationOptions, UNNotificationRequest, UNNotificationResponse,
    UNNotificationSound, UNUserNotificationCenter, UNUserNotificationCenterDelegate,
};
use serde_json::{json, Value};

use crate::events;
use crate::runtime::Shared;

const PREFIX: &str = "app.zephium.webext.";
const MAX_PER_EXTENSION: usize = 32;
const MAX_CATEGORIES: usize = 32;
const MAX_TEXT: usize = 400;

struct Shown {
    shared: RcWeak<Shared>,
    extension: String,
    id: String,
    /// What the notification shows now, for `update` to merge into.
    options: Value,
}

#[derive(Default)]
struct Center {
    shown: HashMap<String, Shown>,
    categories: BTreeMap<Vec<String>, String>,
    next: Cell<u64>,
    delegate: Option<Retained<Delegate>>,
    authorized: Option<bool>,
}

thread_local! {
    static CENTER: RefCell<Center> = RefCell::new(Center::default());
}

/// The center exists only for a bundled app; an unbundled development binary
/// raises on first touch.
fn bundled() -> bool {
    let bundle = NSBundle::mainBundle();
    bundle.bundleIdentifier().is_some() && bundle.bundlePath().to_string().ends_with(".app")
}

fn text(options: &Value, key: &str) -> String {
    options
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .chars()
        .take(MAX_TEXT)
        .collect()
}

pub(crate) fn permission() -> &'static str {
    match CENTER.with(|center| center.borrow().authorized) {
        Some(false) => "denied",
        _ => "granted",
    }
}

/// Shows or replaces notification `id` of `extension`.
pub(crate) fn create(
    shared: &Rc<Shared>,
    extension: &str,
    name: &str,
    message: &Value,
) -> Result<Value, String> {
    if !bundled() {
        return Err("Notifications are unavailable in this build.".into());
    }
    if CENTER.with(|center| center.borrow().authorized) == Some(false) {
        return Err("Notifications are turned off for Zephium.".into());
    }
    let id: String = message
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty() && id.len() <= 500)
        .ok_or("A notification needs an id.")?
        .to_owned();
    let options = message.get("options").cloned().unwrap_or(Value::Null);
    let buttons: Vec<String> = options
        .get("buttons")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(2)
        .map(|button| text(button, "title"))
        .filter(|title| !title.is_empty())
        .collect();
    let identifier = CENTER.with(|center| -> Result<String, String> {
        let mut center = center.borrow_mut();
        let existing = center
            .shown
            .iter()
            .find(|(_, shown)| {
                shown.extension == extension
                    && shown.id == id
                    && std::ptr::eq(shown.shared.as_ptr(), Rc::as_ptr(shared))
            })
            .map(|(key, _)| key.clone());
        if let Some(key) = existing {
            if let Some(shown) = center.shown.get_mut(&key) {
                shown.options = options.clone();
            }
            return Ok(key);
        }
        // Banners that time out into Notification Center are never reported
        // back, so at the cap the extension's oldest one makes room.
        let mut mine: Vec<(u64, String)> = center
            .shown
            .iter()
            .filter(|(_, shown)| shown.extension == extension)
            .filter_map(|(key, _)| Some((key.strip_prefix(PREFIX)?.parse().ok()?, key.clone())))
            .collect();
        if mine.len() >= MAX_PER_EXTENSION {
            mine.sort_unstable();
            let oldest: Vec<String> = mine.into_iter().take(1).map(|(_, key)| key).collect();
            for key in &oldest {
                center.shown.remove(key);
            }
            withdraw(&oldest);
        }
        let number = center.next.get() + 1;
        center.next.set(number);
        let key = format!("{PREFIX}{number}");
        center.shown.insert(
            key.clone(),
            Shown {
                shared: Rc::downgrade(shared),
                extension: extension.to_owned(),
                id: id.clone(),
                options: options.clone(),
            },
        );
        Ok(key)
    })?;
    install_delegate();
    let content = UNMutableNotificationContent::new();
    content.setTitle(&NSString::from_str(&text(&options, "title")));
    content.setSubtitle(&NSString::from_str(name));
    content.setBody(&NSString::from_str(&text(&options, "message")));
    content.setThreadIdentifier(&NSString::from_str(&format!("{PREFIX}{extension}")));
    if options.get("silent").and_then(Value::as_bool) != Some(true) {
        content.setSound(Some(&UNNotificationSound::defaultSound()));
    }
    content.setCategoryIdentifier(&NSString::from_str(&category(&buttons)));
    let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
        &NSString::from_str(&identifier),
        &content,
        None,
    );
    // A notification the system refused never appeared, so it must not keep
    // counting against the extension or show up in `getAll`.
    let delivered = RcBlock::new(move |error: *mut NSError| {
        if error.is_null() {
            return;
        }
        let identifier = identifier.clone();
        dispatch2::DispatchQueue::main().exec_async(move || {
            CENTER.with(|center| center.borrow_mut().shown.remove(&identifier));
        });
    });
    UNUserNotificationCenter::currentNotificationCenter()
        .addNotificationRequest_withCompletionHandler(&request, Some(&delivered));
    Ok(Value::String(id))
}

/// Changes the shown notification `id` of `extension`, keeping the fields the
/// update leaves out. False where it is not showing.
pub(crate) fn update(
    shared: &Rc<Shared>,
    extension: &str,
    name: &str,
    message: &Value,
) -> Result<Value, String> {
    let id = message
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let merged = CENTER.with(|center| {
        let center = center.borrow();
        let shown = center.shown.values().find(|shown| {
            shown.extension == extension
                && shown.id == id
                && std::ptr::eq(shown.shared.as_ptr(), Rc::as_ptr(shared))
        })?;
        let mut merged = shown.options.as_object().cloned().unwrap_or_default();
        if let Some(changes) = message.get("options").and_then(Value::as_object) {
            merged.extend(
                changes
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone())),
            );
        }
        Some(Value::Object(merged))
    });
    let Some(options) = merged else {
        return Ok(Value::Bool(false));
    };
    create(
        shared,
        extension,
        name,
        &json!({ "id": id, "options": options }),
    )?;
    Ok(Value::Bool(true))
}

/// Buttons are a category's actions, so each set of titles has a category.
fn category(buttons: &[String]) -> String {
    CENTER.with(|center| {
        let mut center = center.borrow_mut();
        if let Some(existing) = center.categories.get(buttons) {
            return existing.clone();
        }
        if center.categories.len() >= MAX_CATEGORIES {
            center.categories.clear();
        }
        let identifier = format!("{PREFIX}category.{}", center.categories.len());
        center
            .categories
            .insert(buttons.to_vec(), identifier.clone());
        let categories: Vec<Retained<UNNotificationCategory>> = center
            .categories
            .iter()
            .map(|(titles, identifier)| {
                let actions: Vec<Retained<UNNotificationAction>> = titles
                    .iter()
                    .enumerate()
                    .map(|(index, title)| {
                        UNNotificationAction::actionWithIdentifier_title_options(
                            &NSString::from_str(&format!("button.{index}")),
                            &NSString::from_str(title),
                            UNNotificationActionOptions::empty(),
                        )
                    })
                    .collect();
                UNNotificationCategory::categoryWithIdentifier_actions_intentIdentifiers_options(
                    &NSString::from_str(identifier),
                    &NSArray::from_retained_slice(&actions),
                    &NSArray::new(),
                    UNNotificationCategoryOptions::CustomDismissAction,
                )
            })
            .collect();
        UNUserNotificationCenter::currentNotificationCenter()
            .setNotificationCategories(&NSSet::from_retained_slice(&categories));
        identifier
    })
}

pub(crate) fn clear(shared: &Shared, extension: &str, id: &str) -> bool {
    let removed = CENTER.with(|center| {
        let mut center = center.borrow_mut();
        let keys: Vec<String> = center
            .shown
            .iter()
            .filter(|(_, shown)| {
                shown.extension == extension
                    && shown.id == id
                    && std::ptr::eq(shown.shared.as_ptr(), shared)
            })
            .map(|(key, _)| key.clone())
            .collect();
        for key in &keys {
            center.shown.remove(key);
        }
        keys
    });
    withdraw(&removed);
    !removed.is_empty()
}

pub(crate) fn all(shared: &Shared, extension: &str) -> Value {
    CENTER.with(|center| {
        let ids: serde_json::Map<String, Value> = center
            .borrow()
            .shown
            .values()
            .filter(|shown| {
                shown.extension == extension && std::ptr::eq(shown.shared.as_ptr(), shared)
            })
            .map(|shown| (shown.id.clone(), Value::Bool(true)))
            .collect();
        Value::Object(ids)
    })
}

/// The extension was unloaded; its notifications go with it.
pub(crate) fn forget(shared: &Shared, extension: &str) {
    let removed = CENTER.with(|center| {
        let mut center = center.borrow_mut();
        let keys: Vec<String> = center
            .shown
            .iter()
            .filter(|(_, shown)| {
                shown.extension == extension && std::ptr::eq(shown.shared.as_ptr(), shared)
            })
            .map(|(key, _)| key.clone())
            .collect();
        for key in &keys {
            center.shown.remove(key);
        }
        keys
    });
    withdraw(&removed);
}

fn withdraw(keys: &[String]) {
    if keys.is_empty() || !bundled() {
        return;
    }
    let keys: Vec<Retained<NSString>> = keys.iter().map(|key| NSString::from_str(key)).collect();
    let keys = NSArray::from_retained_slice(&keys);
    let center = UNUserNotificationCenter::currentNotificationCenter();
    center.removeDeliveredNotificationsWithIdentifiers(&keys);
    center.removePendingNotificationRequestsWithIdentifiers(&keys);
}

fn install_delegate() {
    let first = CENTER.with(|center| center.borrow().delegate.is_none());
    if !first {
        return;
    }
    let delegate = Delegate::new();
    let center = UNUserNotificationCenter::currentNotificationCenter();
    center.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    let answered = RcBlock::new(|granted: Bool, _error: *mut NSError| {
        let granted = granted.as_bool();
        dispatch2::DispatchQueue::main().exec_async(move || {
            CENTER.with(|center| center.borrow_mut().authorized = Some(granted));
        });
    });
    center.requestAuthorizationWithOptions_completionHandler(
        UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
        &answered,
    );
    CENTER.with(|center| center.borrow_mut().delegate = Some(delegate));
}

/// A response to one of the browser's notifications, delivered on the main
/// thread where the runtimes live.
fn respond(identifier: String, action: String) {
    let shown = CENTER.with(|center| {
        let mut center = center.borrow_mut();
        if action == "click" || action == "dismiss" {
            center.shown.remove(&identifier)
        } else {
            center.shown.get(&identifier).map(|shown| Shown {
                shared: shown.shared.clone(),
                extension: shown.extension.clone(),
                id: shown.id.clone(),
                options: Value::Null,
            })
        }
    });
    let Some(Shown {
        shared,
        extension,
        id,
        ..
    }) = shown
    else {
        return;
    };
    let Some(shared) = shared.upgrade() else {
        return;
    };
    match action.as_str() {
        "click" => {
            events::emit(
                &shared,
                &extension,
                "notifications.onClicked",
                json!([id]),
                None,
            );
            events::emit(
                &shared,
                &extension,
                "notifications.onClosed",
                json!([id, true]),
                None,
            );
        }
        "dismiss" => events::emit(
            &shared,
            &extension,
            "notifications.onClosed",
            json!([id, true]),
            None,
        ),
        button => {
            if let Some(index) = button
                .strip_prefix("button.")
                .and_then(|i| i.parse::<u8>().ok())
            {
                let args = json!([id, index]);
                events::emit(
                    &shared,
                    &extension,
                    "notifications.onButtonClicked",
                    args,
                    None,
                );
            }
        }
    }
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "ZephiumWebExtNotificationDelegate"]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl UNUserNotificationCenterDelegate for Delegate {
        #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
        fn will_present(
            &self,
            _center: &UNUserNotificationCenter,
            notification: &UNNotification,
            completion: &block2::DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
        ) {
            let ours = notification
                .request()
                .identifier()
                .to_string()
                .starts_with(PREFIX);
            completion.call((if ours {
                UNNotificationPresentationOptions::Banner
                    | UNNotificationPresentationOptions::List
                    | UNNotificationPresentationOptions::Sound
            } else {
                UNNotificationPresentationOptions::empty()
            },));
        }

        #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
        fn did_receive(
            &self,
            _center: &UNUserNotificationCenter,
            response: &UNNotificationResponse,
            completion: &block2::DynBlock<dyn Fn()>,
        ) {
            let identifier = response.notification().request().identifier().to_string();
            let action = response.actionIdentifier();
            let action = if &*action == unsafe { UNNotificationDefaultActionIdentifier } {
                "click".to_owned()
            } else if &*action == unsafe { UNNotificationDismissActionIdentifier } {
                "dismiss".to_owned()
            } else {
                action.to_string()
            };
            if identifier.starts_with(PREFIX) {
                dispatch2::DispatchQueue::main().exec_async(move || respond(identifier, action));
            }
            completion.call(());
        }
    }
);

impl Delegate {
    fn new() -> Retained<Self> {
        let this = Self::alloc().set_ivars(());
        unsafe { msg_send![super(this), init] }
    }
}
