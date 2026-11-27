// Copyright 2026 Zephium contributors
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

use objc2::rc::Retained;
use objc2_foundation::{
  NSString, NSURLAuthenticationChallenge, NSURLAuthenticationMethodDefault,
  NSURLAuthenticationMethodHTTPBasic, NSURLAuthenticationMethodHTTPDigest,
  NSURLAuthenticationMethodNTLM, NSURLAuthenticationMethodNegotiate, NSURLCredential,
  NSURLCredentialPersistence, NSURLSessionAuthChallengeDisposition,
};

use objc2_web_kit::WKWebView;

use crate::native_bounds::{bounded_nsstring, NativeStringLimit, PAGE_URL_LIMIT};
use crate::{HttpAuthChallenge, HttpAuthResponder};

const PROTECTION_SPACE_LIMIT: NativeStringLimit = NativeStringLimit {
  max_utf16_units: 256,
  max_utf8_bytes: 256,
};

type Completion =
  block2::DynBlock<dyn Fn(NSURLSessionAuthChallengeDisposition, *mut NSURLCredential)>;

/// The challenge as the embedder may see it, for the HTTP sign-in methods
/// only. Anything else, or a protection space that is too long or
/// malformed, is None and keeps the cancel path.
pub(crate) fn challenge(
  webview: &WKWebView,
  challenge: &NSURLAuthenticationChallenge,
  method: &NSString,
) -> Option<HttpAuthChallenge> {
  // SAFETY: Foundation's authentication-method constants are immutable
  // process-lifetime strings.
  let sign_in = unsafe {
    [
      NSURLAuthenticationMethodDefault,
      NSURLAuthenticationMethodHTTPBasic,
      NSURLAuthenticationMethodHTTPDigest,
      NSURLAuthenticationMethodNTLM,
      NSURLAuthenticationMethodNegotiate,
    ]
  };
  if !sign_in.iter().any(|known| method.isEqualToString(known)) {
    return None;
  }
  let space = challenge.protectionSpace();
  let proxy = space.isProxy();
  let scheme = if proxy {
    space.proxyType()
  } else {
    space.protocol()
  }?;
  let scheme = bounded_nsstring(&scheme, PROTECTION_SPACE_LIMIT)?;
  let host = bounded_nsstring(&space.host(), PROTECTION_SPACE_LIMIT)?;
  if scheme.is_empty() || host.is_empty() {
    return None;
  }
  let port = match space.port() {
    0 => None,
    port => Some(u16::try_from(port).ok()?),
  };
  let previous_failures =
    u32::try_from(challenge.previousFailureCount().max(0)).unwrap_or(u32::MAX);
  // SAFETY: called on WebKit's main thread with a live web view.
  let main_frame_url = unsafe { webview.URL() }
    .and_then(|url| url.absoluteString())
    .and_then(|url| bounded_nsstring(&url, PAGE_URL_LIMIT));
  Some(HttpAuthChallenge {
    scheme,
    host,
    port,
    proxy,
    previous_failures,
    main_frame_url,
  })
}

/// Holds WebKit's completion until the embedder answers. A credential is
/// kept for this session only, never in the keychain.
pub(crate) fn responder(completion: &Completion) -> HttpAuthResponder {
  let completion = completion.copy();
  HttpAuthResponder::new(move |credentials| {
    let Some((user, password)) = credentials else {
      completion.call((
        NSURLSessionAuthChallengeDisposition::CancelAuthenticationChallenge,
        std::ptr::null_mut(),
      ));
      return;
    };
    let credential = NSURLCredential::credentialWithUser_password_persistence(
      &NSString::from_str(user),
      &NSString::from_str(password),
      NSURLCredentialPersistence::ForSession,
    );
    completion.call((
      NSURLSessionAuthChallengeDisposition::UseCredential,
      Retained::as_ptr(&credential).cast_mut(),
    ));
  })
}
