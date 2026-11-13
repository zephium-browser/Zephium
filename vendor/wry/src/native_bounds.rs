// Copyright 2020-2026 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! Allocation bounds for strings crossing from an untrusted web process.
//!
//! These limits are deliberately enforced before constructing a Rust
//! [`String`]. Platform web engines may still own their native value, but a
//! hostile page must not make the browser UI process duplicate an arbitrarily
//! large title, URL, or filename.

#[cfg(any(gtk, test))]
use std::{ffi::c_char, slice};

#[derive(Clone, Copy)]
pub(crate) struct NativeStringLimit {
  pub(crate) max_utf16_units: usize,
  pub(crate) max_utf8_bytes: usize,
}

// The browser model keeps at most 512 Unicode scalar values. Twice that many
// UTF-16 units preserves 512 non-BMP characters while still bounding the
// native read; the UTF-8 ceiling covers every value admitted by that limit.
pub(crate) const PAGE_TITLE_LIMIT: NativeStringLimit = NativeStringLimit {
  max_utf16_units: 1_024,
  max_utf8_bytes: 4 * 1_024,
};

// Keep this synchronized with Zephium's committed-navigation ceiling. The
// second check is intentional: 8 Ki UTF-16 units can expand past 8 KiB in
// UTF-8, while navigation policy is byte based.
pub(crate) const PAGE_URL_LIMIT: NativeStringLimit = NativeStringLimit {
  max_utf16_units: 8 * 1_024,
  max_utf8_bytes: 8 * 1_024,
};

// Privileged chrome commands are intentionally small (URLs, identifiers,
// settings, and bounded UI mutations). Keep a single cross-platform ceiling
// so a compromised renderer cannot make the UI process duplicate an
// attacker-sized script message. UTF-16 and UTF-8 are checked independently.
pub(crate) const IPC_PAYLOAD_LIMIT: NativeStringLimit = NativeStringLimit {
  max_utf16_units: 64 * 1_024,
  max_utf8_bytes: 64 * 1_024,
};

// The privileged Tauri renderer uses a Wry custom protocol for asset loads
// and small command envelopes. Keep every independently copied field bounded,
// then apply aggregate header/body budgets so a compromised renderer cannot
// turn one request into attacker-sized UI-process allocations.
pub(crate) const CUSTOM_PROTOCOL_METHOD_LIMIT: NativeStringLimit = NativeStringLimit {
  max_utf16_units: 64,
  max_utf8_bytes: 64,
};
pub(crate) const CUSTOM_PROTOCOL_HEADER_NAME_LIMIT: NativeStringLimit = NativeStringLimit {
  max_utf16_units: 1_024,
  max_utf8_bytes: 1_024,
};
pub(crate) const CUSTOM_PROTOCOL_HEADER_VALUE_LIMIT: NativeStringLimit = NativeStringLimit {
  max_utf16_units: 16 * 1_024,
  max_utf8_bytes: 16 * 1_024,
};
pub(crate) const CUSTOM_PROTOCOL_HEADER_COUNT_LIMIT: usize = 128;
pub(crate) const CUSTOM_PROTOCOL_HEADER_BYTES_LIMIT: usize = 64 * 1_024;
pub(crate) const CUSTOM_PROTOCOL_BODY_LIMIT: usize = 64 * 1_024;

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct CustomProtocolRequestBudget {
  header_count: usize,
  header_bytes: usize,
  body_bytes: usize,
}

impl CustomProtocolRequestBudget {
  pub(crate) fn admit_header(&mut self, name_bytes: usize, value_bytes: usize) -> bool {
    if name_bytes > CUSTOM_PROTOCOL_HEADER_NAME_LIMIT.max_utf8_bytes
      || value_bytes > CUSTOM_PROTOCOL_HEADER_VALUE_LIMIT.max_utf8_bytes
    {
      return false;
    }
    let Some(header_count) = self.header_count.checked_add(1) else {
      return false;
    };
    let Some(header_bytes) = self
      .header_bytes
      .checked_add(name_bytes)
      .and_then(|bytes| bytes.checked_add(value_bytes))
    else {
      return false;
    };
    if header_count > CUSTOM_PROTOCOL_HEADER_COUNT_LIMIT
      || header_bytes > CUSTOM_PROTOCOL_HEADER_BYTES_LIMIT
    {
      return false;
    }
    self.header_count = header_count;
    self.header_bytes = header_bytes;
    true
  }

  pub(crate) fn admit_body_chunk(&mut self, bytes: usize) -> bool {
    let Some(body_bytes) = self.body_bytes.checked_add(bytes) else {
      return false;
    };
    if body_bytes > CUSTOM_PROTOCOL_BODY_LIMIT {
      return false;
    }
    self.body_bytes = body_bytes;
    true
  }
}

// A destination component is restricted more tightly than a URL or title.
#[cfg(target_vendor = "apple")]
// A security origin's scheme and host: a DNS name is at most 253 bytes, so
// 512 covers any host WebKit can name.
pub(crate) const ORIGIN_PART_LIMIT: NativeStringLimit = NativeStringLimit {
  max_utf16_units: 512,
  max_utf8_bytes: 512,
};

#[cfg(any(target_os = "macos", target_os = "ios"))]
pub(crate) const DOWNLOAD_FILENAME_LIMIT: NativeStringLimit = NativeStringLimit {
  max_utf16_units: 240,
  max_utf8_bytes: 240,
};

// One file URL from an AppKit pasteboard. macOS paths are much smaller in
// practice, but the URL form may percent-expand; this remains a strict bound
// before copying native NSString contents into Rust.
#[cfg(target_os = "macos")]
pub(crate) const DRAG_DROP_FILE_URL_LIMIT: NativeStringLimit = NativeStringLimit {
  max_utf16_units: 4 * 1_024,
  max_utf8_bytes: 16 * 1_024,
};

#[cfg(any(target_os = "windows", test))]
pub(crate) fn bounded_utf16(units: &[u16], limit: NativeStringLimit) -> Option<String> {
  if units.len() > limit.max_utf16_units {
    return None;
  }
  // Compute the lossy UTF-8 result length without first allocating it. This
  // matches `String::from_utf16_lossy`, including U+FFFD for an unpaired
  // surrogate, and lets the byte ceiling remain a pre-allocation invariant.
  let utf8_len = char::decode_utf16(units.iter().copied()).try_fold(0usize, |length, scalar| {
    length.checked_add(scalar.unwrap_or(char::REPLACEMENT_CHARACTER).len_utf8())
  })?;
  if utf8_len > limit.max_utf8_bytes {
    return None;
  }
  let value = String::from_utf16_lossy(units);
  debug_assert_eq!(value.len(), utf8_len);
  Some(value)
}

#[cfg(target_vendor = "apple")]
pub(crate) fn bounded_nsstring(
  value: &objc2_foundation::NSString,
  limit: NativeStringLimit,
) -> Option<String> {
  if value.length() > limit.max_utf16_units {
    return None;
  }
  // Ask Foundation for the exact UTF-8 size before `NSString::to_string`
  // allocates its Rust-owned copy. Checking UTF-16 units alone is not enough:
  // BMP scalar values can expand to three UTF-8 bytes each.
  if value.lengthOfBytesUsingEncoding(objc2_foundation::NSUTF8StringEncoding) > limit.max_utf8_bytes
  {
    return None;
  }
  let value = value.to_string();
  (value.len() <= limit.max_utf8_bytes).then_some(value)
}

#[cfg(any(gtk, test))]
pub(crate) fn bounded_utf8_bytes(bytes: &[u8], limit: NativeStringLimit) -> Option<String> {
  if bytes.len() > limit.max_utf8_bytes {
    return None;
  }
  std::str::from_utf8(bytes).ok().map(ToOwned::to_owned)
}

#[cfg(any(gtk, test))]
/// Copies a borrowed, NUL-terminated native UTF-8 string after a bounded scan.
///
/// # Safety
///
/// `pointer` must either be null or point to memory readable through the first
/// NUL byte or `limit.max_utf8_bytes + 1` bytes, whichever comes first. The
/// native owner must keep that memory alive for this call.
pub(crate) unsafe fn bounded_utf8_c_string(
  pointer: *const c_char,
  limit: NativeStringLimit,
) -> Option<String> {
  if pointer.is_null() {
    return None;
  }

  let mut length = 0;
  while length <= limit.max_utf8_bytes {
    // SAFETY: guaranteed by the function contract; the scan never exceeds the
    // documented bound plus the byte needed to distinguish exact-bound values.
    if unsafe { pointer.add(length).read() } == 0 {
      // SAFETY: the bounded scan proved this many initialized bytes precede the
      // terminator and the native owner remains alive for this call.
      let bytes = unsafe { slice::from_raw_parts(pointer.cast::<u8>(), length) };
      return bounded_utf8_bytes(bytes, limit);
    }
    length += 1;
  }
  None
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::ffi::CString;

  const SMALL: NativeStringLimit = NativeStringLimit {
    max_utf16_units: 4,
    max_utf8_bytes: 4,
  };

  #[test]
  fn utf16_checks_native_units_and_encoded_bytes() {
    assert_eq!(
      bounded_utf16(&[b'a' as u16; 4], SMALL).as_deref(),
      Some("aaaa")
    );
    assert!(bounded_utf16(&[b'a' as u16; 5], SMALL).is_none());
    // Two BMP scalar values fit the native-unit limit but expand beyond four
    // bytes in UTF-8, so the post-conversion byte check rejects them.
    assert!(bounded_utf16(&[0x6c34, 0x6c34], SMALL).is_none());
  }

  #[test]
  fn utf8_pointer_scan_accepts_exact_bound_only_with_terminator() {
    let exact = CString::new("aaaa").unwrap();
    assert_eq!(
      unsafe { bounded_utf8_c_string(exact.as_ptr(), SMALL) }.as_deref(),
      Some("aaaa")
    );

    let over = CString::new("aaaaa").unwrap();
    assert!(unsafe { bounded_utf8_c_string(over.as_ptr(), SMALL) }.is_none());
    assert!(unsafe { bounded_utf8_c_string(std::ptr::null(), SMALL) }.is_none());
  }

  #[test]
  fn utf8_pointer_scan_rejects_invalid_utf8_without_lossy_expansion() {
    let invalid = [0xff_u8, 0];
    assert!(unsafe { bounded_utf8_c_string(invalid.as_ptr().cast(), SMALL) }.is_none());
  }

  #[test]
  fn ipc_payload_accepts_exact_bound_and_rejects_one_unit_or_byte_over() {
    let exact_utf16 = vec![b'a' as u16; IPC_PAYLOAD_LIMIT.max_utf16_units];
    assert_eq!(
      bounded_utf16(&exact_utf16, IPC_PAYLOAD_LIMIT)
        .as_deref()
        .map(str::len),
      Some(IPC_PAYLOAD_LIMIT.max_utf8_bytes)
    );
    let over_utf16 = vec![b'a' as u16; IPC_PAYLOAD_LIMIT.max_utf16_units + 1];
    assert!(bounded_utf16(&over_utf16, IPC_PAYLOAD_LIMIT).is_none());

    let exact_utf8 = vec![b'a'; IPC_PAYLOAD_LIMIT.max_utf8_bytes];
    assert_eq!(
      bounded_utf8_bytes(&exact_utf8, IPC_PAYLOAD_LIMIT)
        .as_deref()
        .map(str::len),
      Some(IPC_PAYLOAD_LIMIT.max_utf8_bytes)
    );
    let over_utf8 = vec![b'a'; IPC_PAYLOAD_LIMIT.max_utf8_bytes + 1];
    assert!(bounded_utf8_bytes(&over_utf8, IPC_PAYLOAD_LIMIT).is_none());

    // Native-unit admission alone is insufficient: BMP characters fit the
    // UTF-16 ceiling but expand beyond the shared UTF-8 byte budget.
    let expanding = vec![0x6c34; IPC_PAYLOAD_LIMIT.max_utf16_units];
    assert!(bounded_utf16(&expanding, IPC_PAYLOAD_LIMIT).is_none());
  }

  #[test]
  fn custom_protocol_budget_accepts_exact_limits_and_rejects_one_over() {
    let exact_method = vec![b'P'; CUSTOM_PROTOCOL_METHOD_LIMIT.max_utf8_bytes];
    assert!(bounded_utf8_bytes(&exact_method, CUSTOM_PROTOCOL_METHOD_LIMIT).is_some());
    let over_method = vec![b'P'; CUSTOM_PROTOCOL_METHOD_LIMIT.max_utf8_bytes + 1];
    assert!(bounded_utf8_bytes(&over_method, CUSTOM_PROTOCOL_METHOD_LIMIT).is_none());

    let mut exact_headers = CustomProtocolRequestBudget::default();
    for _ in
      0..(CUSTOM_PROTOCOL_HEADER_BYTES_LIMIT / CUSTOM_PROTOCOL_HEADER_VALUE_LIMIT.max_utf8_bytes)
    {
      assert!(exact_headers.admit_header(0, CUSTOM_PROTOCOL_HEADER_VALUE_LIMIT.max_utf8_bytes));
    }
    assert!(!exact_headers.admit_header(1, 0));

    let mut exact_count = CustomProtocolRequestBudget::default();
    for _ in 0..CUSTOM_PROTOCOL_HEADER_COUNT_LIMIT {
      assert!(exact_count.admit_header(0, 0));
    }
    assert!(!exact_count.admit_header(0, 0));

    let mut over_field = CustomProtocolRequestBudget::default();
    assert!(!over_field.admit_header(CUSTOM_PROTOCOL_HEADER_NAME_LIMIT.max_utf8_bytes + 1, 0));
    assert!(!over_field.admit_header(0, CUSTOM_PROTOCOL_HEADER_VALUE_LIMIT.max_utf8_bytes + 1));

    let mut exact_body = CustomProtocolRequestBudget::default();
    assert!(exact_body.admit_body_chunk(CUSTOM_PROTOCOL_BODY_LIMIT));
    assert!(!exact_body.admit_body_chunk(1));
    let mut over_body = CustomProtocolRequestBudget::default();
    assert!(!over_body.admit_body_chunk(CUSTOM_PROTOCOL_BODY_LIMIT + 1));
  }

  #[cfg(target_vendor = "apple")]
  #[test]
  fn nsstring_ipc_bound_is_checked_before_copying_to_rust() {
    use objc2_foundation::NSString;

    let exact = NSString::from_str(&"a".repeat(IPC_PAYLOAD_LIMIT.max_utf16_units));
    assert_eq!(
      bounded_nsstring(&exact, IPC_PAYLOAD_LIMIT)
        .as_deref()
        .map(str::len),
      Some(IPC_PAYLOAD_LIMIT.max_utf8_bytes)
    );

    let over = NSString::from_str(&"a".repeat(IPC_PAYLOAD_LIMIT.max_utf16_units + 1));
    assert!(bounded_nsstring(&over, IPC_PAYLOAD_LIMIT).is_none());

    // Foundation reports the exact native UTF-8 size before the Rust-owned
    // copy is made, so a value that fits in UTF-16 but exceeds the byte budget
    // is rejected as well.
    let expanding = NSString::from_str(&"水".repeat(IPC_PAYLOAD_LIMIT.max_utf16_units));
    assert!(bounded_nsstring(&expanding, IPC_PAYLOAD_LIMIT).is_none());
  }
}
