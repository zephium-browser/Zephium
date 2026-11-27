// Copyright 2026 Zephium contributors
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

pub(crate) type HttpAuthHandler = dyn Fn(HttpAuthChallenge, HttpAuthResponder);
type Completion = Box<dyn FnOnce(Option<(&str, &str)>)>;

/// An HTTP Basic, Digest, NTLM or Negotiate challenge from the protection
/// space WebKit reported. The server's realm is deliberately absent: it is
/// page-controlled text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpAuthChallenge {
  /// `http` or `https` for a server; the proxy type for a proxy.
  pub scheme: String,
  pub host: String,
  pub port: Option<u16>,
  pub proxy: bool,
  /// WebKit's count of earlier failed attempts for this protection space.
  pub previous_failures: u32,
  /// The web view's main-frame address when the challenge arrived: the
  /// provisional one while a navigation loads.
  pub main_frame_url: Option<String>,
}

/// Single-use, main-thread completion of one challenge. Dropping it cancels.
///
/// Neither Clone nor Send. Credentials are copied into an `NSURLCredential`
/// with session persistence and are not retained by this type.
pub struct HttpAuthResponder {
  completion: Option<Completion>,
}

impl HttpAuthResponder {
  pub(crate) fn new(completion: impl FnOnce(Option<(&str, &str)>) + 'static) -> Self {
    Self {
      completion: Some(Box::new(completion)),
    }
  }

  /// Sign in with `(user, password)`, or cancel with None.
  pub fn respond(mut self, credentials: Option<(&str, &str)>) {
    if let Some(completion) = self.completion.take() {
      completion(credentials);
    }
  }
}

impl Drop for HttpAuthResponder {
  fn drop(&mut self) {
    if let Some(completion) = self.completion.take() {
      completion(None);
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::{cell::Cell, rc::Rc};

  #[test]
  fn a_responder_settles_once_and_cancels_when_abandoned() {
    for answer in [Some(true), Some(false), None] {
      let calls = Rc::new(Cell::new(0));
      let signed_in = Rc::new(Cell::new(false));
      let (seen, seen_sign_in) = (calls.clone(), signed_in.clone());
      let responder = HttpAuthResponder::new(move |credentials| {
        seen.set(seen.get() + 1);
        seen_sign_in.set(credentials == Some(("user", "secret")));
      });
      match answer {
        Some(true) => responder.respond(Some(("user", "secret"))),
        Some(false) => responder.respond(None),
        None => drop(responder),
      }
      assert_eq!(calls.get(), 1);
      assert_eq!(signed_in.get(), answer == Some(true));
    }
  }

  #[test]
  fn sign_in_requires_explicit_construction_opt_in() {
    use crate::{WebViewBuilder, WebViewBuilderExtMacos};
    assert!(WebViewBuilder::new()
      .platform_specific
      .http_auth_handler
      .is_none());
    assert!(WebViewBuilder::new()
      .with_http_auth_handler(|_, _| {})
      .platform_specific
      .http_auth_handler
      .is_some());
  }
}
