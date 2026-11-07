// Copyright 2026 Zephium contributors
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! Server certificates: what was wrong with one WebKit refused, and an
//! embedder's narrow exception for one it accepted. Classification evaluates
//! a fresh trust built from the presented chain with network fetches off, so
//! it never blocks on a page-chosen revocation or issuer URL.

use std::ptr::NonNull;

use objc2::{msg_send, rc::Retained, runtime::AnyObject, ClassType};
use objc2_core_foundation::{CFArray, CFError, CFRetained, CFString, CFType};
use objc2_foundation::{
  NSError, NSURLAuthenticationChallenge, NSURLCredential, NSURLErrorFailingURLErrorKey,
  NSURLErrorFailingURLPeerTrustErrorKey, NSURL,
};
use objc2_security::{
  errSecCertificateExpired, errSecCertificateNotValidYet, errSecCertificateRevoked,
  errSecCreateChainFailed, errSecHostNameMismatch, errSecInvalidCertAuthority, errSecInvalidRoot,
  errSecNoBasicConstraints, errSecNotTrusted, errSecVerifyActionFailed, SecCertificate, SecPolicy,
  SecTrust,
};
use sha2::{Digest, Sha256};

use crate::native_bounds::{bounded_nsstring, NativeStringLimit, PAGE_URL_LIMIT};
use crate::{CertificateFailure, CertificateProblem, ServerTrustExceptions};

const HOST_LIMIT: NativeStringLimit = NativeStringLimit {
  max_utf16_units: 256,
  max_utf8_bytes: 256,
};

/// The certificate errors WebKit reports for a refused server chain: bad
/// date, untrusted, unknown root, not yet valid.
fn is_certificate_error(code: isize) -> bool {
  (-1204..=-1201).contains(&code)
}

fn code_problem(code: isize) -> CertificateProblem {
  match code {
    -1201 | -1204 => CertificateProblem::Expired,
    -1203 => CertificateProblem::Untrusted,
    _ => CertificateProblem::Other,
  }
}

fn status_problem(status: i32) -> CertificateProblem {
  match status {
    s if s == errSecCertificateExpired || s == errSecCertificateNotValidYet => {
      CertificateProblem::Expired
    }
    s if s == errSecHostNameMismatch => CertificateProblem::NameMismatch,
    s if s == errSecCertificateRevoked => CertificateProblem::Revoked,
    s if s == errSecNotTrusted
      || s == errSecCreateChainFailed
      || s == errSecInvalidRoot
      || s == errSecInvalidCertAuthority
      || s == errSecNoBasicConstraints
      || s == errSecVerifyActionFailed =>
    {
      CertificateProblem::Untrusted
    }
    _ => CertificateProblem::Other,
  }
}

fn chain(trust: &SecTrust) -> Option<CFRetained<CFArray<SecCertificate>>> {
  // SAFETY: a live trust; the copied array holds SecCertificate values.
  let chain = unsafe { trust.certificate_chain() }?;
  (!chain.is_empty()).then(|| unsafe { CFRetained::cast_unchecked(chain) })
}

fn leaf_sha256(chain: &CFArray<SecCertificate>) -> Option<[u8; 32]> {
  let leaf = chain.get(0)?;
  // SAFETY: a live certificate; the copied data is its DER encoding.
  let der = unsafe { leaf.data() };
  Some(Sha256::digest(der.to_vec()).into())
}

/// None when the chain is trusted for `host`; otherwise what is wrong.
fn evaluate(chain: &CFArray<SecCertificate>, host: &str) -> Option<CertificateProblem> {
  let host = CFString::from_str(host);
  // SAFETY: SecPolicyCreateSSL with a valid host string.
  let policy = unsafe { SecPolicy::new_ssl(true, Some(&host)) };
  let mut raw: *mut SecTrust = std::ptr::null_mut();
  // SAFETY: certificates is a CFArray of SecCertificate and policies a single
  // SecPolicy; on success `raw` is a +1 SecTrust we take ownership of.
  let status = unsafe {
    SecTrust::create_with_certificates(
      chain.as_ref(),
      Some(policy.as_ref()),
      NonNull::from(&mut raw),
    )
  };
  let Some(raw) = NonNull::new(raw).filter(|_| status == 0) else {
    return Some(CertificateProblem::Other);
  };
  // SAFETY: created above with a +1 retain.
  let trust = unsafe { CFRetained::from_raw(raw) };
  // SAFETY: a fresh trust owned here. Evaluating with network fetches on
  // could block this thread on a page-chosen URL, so refuse to evaluate.
  if unsafe { trust.set_network_fetch_allowed(false) } != 0 {
    return Some(CertificateProblem::Other);
  }
  let mut error: *mut CFError = std::ptr::null_mut();
  // SAFETY: `error` is a valid out pointer.
  let trusted = unsafe { trust.evaluate_with_error(&mut error) };
  let error = NonNull::new(error).map(|error| unsafe { CFRetained::from_raw(error) });
  if trusted {
    return None;
  }
  Some(error.map_or(CertificateProblem::Other, |error| {
    i32::try_from(error.code()).map_or(CertificateProblem::Other, status_problem)
  }))
}

fn trust_from_object(object: &AnyObject) -> Option<&SecTrust> {
  // SAFETY: every CoreFoundation object is also an Objective-C object;
  // the type is checked before use.
  let value = unsafe { &*(object as *const AnyObject).cast::<CFType>() };
  value.downcast_ref::<SecTrust>()
}

/// A refused main-frame load, when WebKit's error is a certificate error.
pub(crate) fn certificate_failure(error: &NSError, code: isize) -> Option<CertificateFailure> {
  if !is_certificate_error(code) {
    return None;
  }
  let info = error.userInfo();
  let url = info
    // SAFETY: Foundation's immutable error-key constants.
    .objectForKey(unsafe { NSURLErrorFailingURLErrorKey })
    .and_then(|value| value.downcast::<NSURL>().ok())?;
  let host = url
    .host()
    .and_then(|host| bounded_nsstring(&host, HOST_LIMIT))?;
  let url = url
    .absoluteString()
    .and_then(|url| bounded_nsstring(&url, PAGE_URL_LIMIT))?;
  let peer = info.objectForKey(unsafe { NSURLErrorFailingURLPeerTrustErrorKey });
  let chain = peer.as_deref().and_then(trust_from_object).and_then(chain);
  let (problem, leaf_sha256) = match chain {
    Some(chain) => (
      evaluate(&chain, &host).unwrap_or(CertificateProblem::Other),
      leaf_sha256(&chain),
    ),
    None => (code_problem(code), None),
  };
  Some(CertificateFailure {
    url,
    problem,
    leaf_sha256,
  })
}

/// A credential for a server-trust challenge the embedder's exception
/// accepts: same origin, same leaf, and system evaluation still failing.
pub(crate) fn excepted(
  exceptions: &ServerTrustExceptions,
  challenge: &NSURLAuthenticationChallenge,
) -> Option<Retained<NSURLCredential>> {
  let space = challenge.protectionSpace();
  if space.isProxy() {
    return None;
  }
  let scheme = bounded_nsstring(&*space.protocol()?, HOST_LIMIT)?;
  let host = bounded_nsstring(&space.host(), HOST_LIMIT)?;
  let port = u16::try_from(space.port()).ok().filter(|port| *port != 0)?;
  if !(exceptions.covers)(&scheme, &host, port) {
    return None;
  }
  // SAFETY: `serverTrust` is a plain getter returning an unretained
  // SecTrustRef, or nil, for this protection space.
  let trust: *const std::ffi::c_void = unsafe { msg_send![&*space, serverTrust] };
  // SAFETY: borrowed for the duration of this delegate callback.
  let trust = unsafe { trust.cast::<SecTrust>().as_ref() }?;
  let chain = chain(trust)?;
  let leaf = leaf_sha256(&chain)?;
  if !(exceptions.allows)(&scheme, &host, port, &leaf) || evaluate(&chain, &host).is_none() {
    return None;
  }
  // SAFETY: +credentialForTrust: returns an autoreleased credential for a
  // live trust.
  let credential: Retained<NSURLCredential> = unsafe {
    msg_send![NSURLCredential::class(), credentialForTrust: (trust as *const SecTrust).cast::<std::ffi::c_void>()]
  };
  Some(credential)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn only_refused_chains_count_as_certificate_errors() {
    for code in [-1201, -1202, -1203, -1204] {
      assert!(is_certificate_error(code));
    }
    for code in [-1200, -1205, -1206, -1009, -999] {
      assert!(!is_certificate_error(code));
    }
    assert_eq!(code_problem(-1201), CertificateProblem::Expired);
    assert_eq!(code_problem(-1204), CertificateProblem::Expired);
    assert_eq!(code_problem(-1203), CertificateProblem::Untrusted);
    assert_eq!(code_problem(-1202), CertificateProblem::Other);
  }

  #[test]
  fn security_statuses_name_the_problem() {
    assert_eq!(status_problem(-67818), CertificateProblem::Expired);
    assert_eq!(status_problem(-67819), CertificateProblem::Expired);
    assert_eq!(status_problem(-67602), CertificateProblem::NameMismatch);
    assert_eq!(status_problem(-67820), CertificateProblem::Revoked);
    assert_eq!(status_problem(-67843), CertificateProblem::Untrusted);
    assert_eq!(status_problem(-1), CertificateProblem::Other);
  }
}
