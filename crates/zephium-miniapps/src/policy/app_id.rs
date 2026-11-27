use ring::digest::{digest, SHA256};

/// A stable, content-derived installation id. Reinstalling the same start
/// address updates one record rather than silently creating a twin.
pub fn app_id(start_url: &str) -> String {
    let hash = digest(&SHA256, start_url.as_bytes());
    let mut id = String::with_capacity(32);
    for byte in &hash.as_ref()[..16] {
        use std::fmt::Write as _;
        let _ = write!(id, "{byte:02x}");
    }
    id
}
