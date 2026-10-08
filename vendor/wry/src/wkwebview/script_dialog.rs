// Copyright 2020-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! Page `alert`, `confirm` and `prompt` as a sheet on the window, named by the
//! origin that asked. WKWebView has no dialog surface of its own, and an
//! answer nobody saw (a `confirm` that returns false) breaks every "Are you
//! sure?" flow on the web.

use std::{
  cell::{Cell, RefCell},
  rc::Rc,
  time::{Duration, Instant},
};

use block2::RcBlock;
use objc2::{rc::Retained, MainThreadMarker};
use objc2_app_kit::{
  NSAlert, NSAlertFirstButtonReturn, NSControlStateValueOn, NSModalResponse, NSTextField,
};
use objc2_foundation::{ns_string, NSPoint, NSRect, NSSize, NSString};
use objc2_web_kit::{WKFrameInfo, WKWebView};

const MAX_MESSAGE_CHARS: usize = 1024;
/// A second dialog this soon after the last offers to silence the page.
const REPEAT_WINDOW: Duration = Duration::from_secs(10);

pub(crate) enum Kind {
  Alert,
  Confirm,
  Prompt(String),
}

pub(crate) enum Answer {
  Dismissed,
  Accepted(Option<String>),
}

#[derive(Default)]
pub(crate) struct ScriptDialogs {
  last: Cell<Option<Instant>>,
  /// The document a person silenced. It stays silent until it navigates.
  silenced: Rc<RefCell<Option<String>>>,
}

impl ScriptDialogs {
  pub(crate) fn present(
    &self,
    webview: &WKWebView,
    frame: &WKFrameInfo,
    kind: Kind,
    message: &NSString,
    done: Box<dyn FnOnce(Answer)>,
  ) {
    let Some(mtm) = MainThreadMarker::new() else {
      return done(Answer::Dismissed);
    };
    let page = unsafe { webview.URL() }
      .and_then(|url| url.absoluteString())
      .map(|url| url.to_string());
    if page.is_some() && *self.silenced.borrow() == page {
      return done(Answer::Dismissed);
    }
    // A hidden tab cannot be asked; its page gets the answer of a person
    // who said no, as before. One sheet at a time per window.
    let Some(window) = webview.window().filter(|window| {
      window.isVisible() && window.attachedSheet().is_none() && !webview.isHiddenOrHasHiddenAncestor()
    }) else {
      return done(Answer::Dismissed);
    };
    let now = Instant::now();
    let repeat = self
      .last
      .get()
      .is_some_and(|last| now.duration_since(last) < REPEAT_WINDOW);
    self.last.set(Some(now));

    let alert = NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str(&title(frame)));
    alert.setInformativeText(&NSString::from_str(&bounded(&message.to_string())));
    alert.addButtonWithTitle(ns_string!("OK"));
    if !matches!(kind, Kind::Alert) {
      alert.addButtonWithTitle(ns_string!("Cancel"));
    }
    let field: Option<Retained<NSTextField>> = match &kind {
      Kind::Prompt(default) => {
        let field = NSTextField::textFieldWithString(&NSString::from_str(default), mtm);
        field.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(280.0, 24.0)));
        alert.setAccessoryView(Some(&field));
        Some(field)
      }
      _ => None,
    };
    if repeat {
      alert.setShowsSuppressionButton(true);
      if let Some(button) = alert.suppressionButton() {
        button.setTitle(ns_string!("Don’t allow more dialogs from this page"));
      }
    }

    let done = Cell::new(Some(done));
    let silenced = self.silenced.clone();
    let shown = alert.clone();
    let handler = RcBlock::new(move |response: NSModalResponse| {
      let Some(done) = done.take() else {
        return;
      };
      if repeat
        && shown
          .suppressionButton()
          .is_some_and(|button| button.state() == NSControlStateValueOn)
      {
        *silenced.borrow_mut() = page.clone();
      }
      done(if response == NSAlertFirstButtonReturn {
        Answer::Accepted(field.as_ref().map(|field| field.stringValue().to_string()))
      } else {
        Answer::Dismissed
      });
    });
    alert.beginSheetModalForWindow_completionHandler(&window, Some(&handler));
  }
}

/// Who is speaking, the way other browsers say it: the frame's own host, and
/// that it is embedded when it is not the page itself.
fn title(frame: &WKFrameInfo) -> String {
  let host = unsafe { frame.securityOrigin().host() }.to_string();
  match (host.is_empty(), unsafe { frame.isMainFrame() }) {
    (true, _) => "This page says".to_owned(),
    (false, true) => format!("{host} says"),
    (false, false) => format!("An embedded page at {host} says"),
  }
}

fn bounded(message: &str) -> String {
  let mut chars = message.chars();
  let mut text: String = chars.by_ref().take(MAX_MESSAGE_CHARS).collect();
  if chars.next().is_some() {
    text.push('…');
  }
  text
}
