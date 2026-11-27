// Copyright 2020-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! Page `alert`, `confirm` and `prompt` as a sheet on the window, named by the
//! origin that asked. WKWebView has no dialog surface of its own, and an
//! answer nobody saw (a `confirm` that returns false) breaks every "Are you
//! sure?" flow on the web.

use std::{
  cell::Cell,
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

use crate::native_bounds::{bounded_nsstring, NativeStringLimit, PAGE_TITLE_LIMIT};

const MAX_MESSAGE_CHARS: usize = 1024;
const DIALOG_TEXT_LIMIT: NativeStringLimit = NativeStringLimit {
  max_utf16_units: 2048,
  max_utf8_bytes: 4096,
};
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
  document: Rc<Cell<u64>>,
  /// A committed-document generation, unaffected by URL/history changes.
  silenced: Rc<Cell<Option<u64>>>,
}

fn silence_document(current: &Cell<u64>, silenced: &Cell<Option<u64>>, shown: u64) {
  // A sheet may finish after its page navigated. It must neither silence nor
  // erase an already silenced replacement document.
  if current.get() == shown {
    silenced.set(Some(shown));
  }
}

impl ScriptDialogs {
  pub(crate) fn document_committed(&self) {
    self.last.set(None);
    match self.document.get().checked_add(1) {
      Some(next) => self.document.set(next),
      // Exhaustion must not let a new document reuse a silenced identity.
      None => self.silenced.set(Some(self.document.get())),
    }
  }

  fn is_silenced(&self) -> bool {
    self.silenced.get() == Some(self.document.get())
  }

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
    let Some(message) = dialog_text(message) else {
      return done(Answer::Dismissed);
    };
    let document = self.document.get();
    if self.is_silenced() {
      return done(Answer::Dismissed);
    }
    // A hidden tab cannot be asked; its page gets the answer of a person
    // who said no, as before. One sheet at a time per window.
    let Some(window) = webview.window().filter(|window| {
      window.isVisible()
        && window.attachedSheet().is_none()
        && !webview.isHiddenOrHasHiddenAncestor()
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
    alert.setInformativeText(&NSString::from_str(&bounded(&message)));
    alert.addButtonWithTitle(ns_string!("OK"));
    if !matches!(kind, Kind::Alert) {
      alert.addButtonWithTitle(ns_string!("Cancel"));
    }
    let field: Option<Retained<NSTextField>> = match &kind {
      Kind::Prompt(default) => {
        let field = NSTextField::textFieldWithString(&NSString::from_str(default), mtm);
        field.setFrame(NSRect::new(
          NSPoint::new(0.0, 0.0),
          NSSize::new(280.0, 24.0),
        ));
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
    let current_document = self.document.clone();
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
        silence_document(&current_document, &silenced, document);
      }
      done(if response == NSAlertFirstButtonReturn {
        match &field {
          Some(field) => dialog_text(&field.stringValue())
            .map(|text| Answer::Accepted(Some(text)))
            .unwrap_or(Answer::Dismissed),
          None => Answer::Accepted(None),
        }
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
  let native_host = unsafe { frame.securityOrigin().host() };
  let host = bounded_nsstring(&native_host, PAGE_TITLE_LIMIT).unwrap_or_default();
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

/// Copies a page-owned dialog string.
pub(crate) fn dialog_text(value: &NSString) -> Option<String> {
  bounded_nsstring(value, DIALOG_TEXT_LIMIT)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn silence_follows_the_committed_document_and_ignores_stale_sheet_completion() {
    let dialogs = ScriptDialogs::default();
    dialogs.document_committed();
    let first = dialogs.document.get();
    silence_document(&dialogs.document, &dialogs.silenced, first);
    assert!(dialogs.is_silenced());
    // Hash and pushState changes invoke no main-document commit and cannot
    // alter the suppression identity.
    assert!(dialogs.is_silenced());
    dialogs.document_committed();
    assert!(!dialogs.is_silenced());
    silence_document(&dialogs.document, &dialogs.silenced, first);
    assert!(!dialogs.is_silenced());
    silence_document(&dialogs.document, &dialogs.silenced, dialogs.document.get());
    assert!(dialogs.is_silenced());
    // Completing the old page's sheet after the replacement was silenced
    // must not overwrite that newer suppression with the old generation.
    silence_document(&dialogs.document, &dialogs.silenced, first);
    assert!(dialogs.is_silenced());
  }

  #[test]
  fn native_dialog_inputs_are_bounded_before_copying() {
    assert_eq!(
      dialog_text(&NSString::from_str("hello")).as_deref(),
      Some("hello")
    );
    for value in ["a".repeat(4097), "水".repeat(2048), "😀".repeat(2049)] {
      assert!(dialog_text(&NSString::from_str(&value)).is_none());
    }
    assert!(dialog_text(&NSString::from_str(&"😀".repeat(1024))).is_some());
  }
}
