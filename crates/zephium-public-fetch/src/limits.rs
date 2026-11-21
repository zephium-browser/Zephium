use std::time::Duration;

pub const MAX_REDIRECTS: u8 = 3;
pub const MAX_LOCATION_BYTES: usize = 4096;
pub const MAX_HEADER_BYTES: usize = 64 * 1024;
pub const MAX_DECODED_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_WIRE_BYTES: u64 = 16 * 1024 * 1024;
pub const MAX_CONCURRENT_READS: usize = 16;
pub const MAX_READS_PER_HOST: usize = 4;
pub const MAX_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
pub const MAX_FIRST_BYTE_TIMEOUT: Duration = Duration::from_secs(15);
pub const MAX_READ_DURATION: Duration = Duration::from_secs(30);
pub const SESSION_IDLE_TIMEOUT: Duration = Duration::from_secs(10);
pub const SESSION_IDLE_PER_HOST: usize = 2;
pub const MAX_BROTLI_WINDOW_BITS: u8 = 22;
pub const MAX_GZIP_HEADER_FIELD: usize = 1024;
pub const SNIFF_BYTES: usize = 512;
pub const MAX_MEDIA_TYPE_BYTES: usize = 127;
pub const MAX_CHARSET_BYTES: usize = 40;
pub const RESOLVER_SLOTS_PER_READ: usize = 2;
/// How often the operating system's proxy setting is read again.
pub const SYSTEM_ROUTE_REFRESH: Duration = Duration::from_secs(30);
/// A system read slower than this leaves the route unsupported until it returns.
pub const SYSTEM_ROUTE_TIMEOUT: Duration = Duration::from_secs(2);
/// The least time between the starts of two system reads a reader asked for.
pub const SYSTEM_ROUTE_MIN_GAP: Duration = Duration::from_secs(5);

/// The reader's own bounds. Each field is clamped to its ceiling above and to
/// at least one unit, so a caller can only make the reader stricter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReaderLimits {
    pub max_concurrent: usize,
    pub max_per_host: usize,
    pub connect_timeout: Duration,
    pub first_byte_timeout: Duration,
    pub max_duration: Duration,
    pub max_decoded_bytes: usize,
    pub max_wire_bytes: u64,
    pub max_header_bytes: usize,
}

impl Default for ReaderLimits {
    fn default() -> Self {
        Self {
            max_concurrent: 8,
            max_per_host: 2,
            connect_timeout: Duration::from_secs(4),
            first_byte_timeout: Duration::from_secs(8),
            max_duration: Duration::from_secs(15),
            max_decoded_bytes: 4 * 1024 * 1024,
            max_wire_bytes: 8 * 1024 * 1024,
            max_header_bytes: MAX_HEADER_BYTES,
        }
    }
}

impl ReaderLimits {
    pub(crate) fn clamped(self) -> Self {
        let millisecond = Duration::from_millis(1);
        Self {
            max_concurrent: self.max_concurrent.clamp(1, MAX_CONCURRENT_READS),
            max_per_host: self.max_per_host.clamp(1, MAX_READS_PER_HOST),
            connect_timeout: self.connect_timeout.clamp(millisecond, MAX_CONNECT_TIMEOUT),
            first_byte_timeout: self
                .first_byte_timeout
                .clamp(millisecond, MAX_FIRST_BYTE_TIMEOUT),
            max_duration: self.max_duration.clamp(millisecond, MAX_READ_DURATION),
            max_decoded_bytes: self.max_decoded_bytes.clamp(1, MAX_DECODED_BYTES),
            max_wire_bytes: self.max_wire_bytes.clamp(1, MAX_WIRE_BYTES),
            max_header_bytes: self.max_header_bytes.clamp(1, MAX_HEADER_BYTES),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_caller_can_only_tighten_the_ceilings() {
        let loose = ReaderLimits {
            max_concurrent: usize::MAX,
            max_per_host: usize::MAX,
            connect_timeout: Duration::MAX,
            first_byte_timeout: Duration::MAX,
            max_duration: Duration::MAX,
            max_decoded_bytes: usize::MAX,
            max_wire_bytes: u64::MAX,
            max_header_bytes: usize::MAX,
        }
        .clamped();
        assert_eq!(loose.max_concurrent, MAX_CONCURRENT_READS);
        assert_eq!(loose.max_per_host, MAX_READS_PER_HOST);
        assert_eq!(loose.connect_timeout, MAX_CONNECT_TIMEOUT);
        assert_eq!(loose.first_byte_timeout, MAX_FIRST_BYTE_TIMEOUT);
        assert_eq!(loose.max_duration, MAX_READ_DURATION);
        assert_eq!(loose.max_decoded_bytes, MAX_DECODED_BYTES);
        assert_eq!(loose.max_wire_bytes, MAX_WIRE_BYTES);
        assert_eq!(loose.max_header_bytes, MAX_HEADER_BYTES);
        let zero = ReaderLimits {
            max_concurrent: 0,
            max_per_host: 0,
            connect_timeout: Duration::ZERO,
            first_byte_timeout: Duration::ZERO,
            max_duration: Duration::ZERO,
            max_decoded_bytes: 0,
            max_wire_bytes: 0,
            max_header_bytes: 0,
        }
        .clamped();
        assert_eq!(zero.max_concurrent, 1);
        assert_eq!(zero.max_decoded_bytes, 1);
        assert!(zero.max_duration > Duration::ZERO);
        assert_eq!(ReaderLimits::default().clamped(), ReaderLimits::default());
    }
}
