//! SHA-256 through ring, which uses the CPU's SHA instructions (ARMv8 SHA2,
//! x86 SHA-NI) where they exist. The portable implementation took about
//! 130 ms per 27 MB cache record on an M4.

/// An incremental SHA-256 digest.
#[derive(Clone)]
pub struct Sha256(ring::digest::Context);

impl Default for Sha256 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha256 {
    /// An empty digest.
    pub fn new() -> Self {
        Self(ring::digest::Context::new(&ring::digest::SHA256))
    }

    /// Adds `data` to the digest.
    pub fn update(&mut self, data: impl AsRef<[u8]>) {
        self.0.update(data.as_ref());
    }

    /// The finished digest.
    pub fn finalize(self) -> [u8; 32] {
        let digest = self.0.finish();
        let mut bytes = [0; 32];
        bytes.copy_from_slice(digest.as_ref());
        bytes
    }

    /// The digest of `data`.
    pub fn digest(data: impl AsRef<[u8]>) -> [u8; 32] {
        let mut digest = Self::new();
        digest.update(data);
        digest.finalize()
    }
}

#[cfg(test)]
mod tests {
    use super::Sha256;

    #[test]
    fn matches_the_standard_vectors() {
        let hex = |bytes: [u8; 32]| bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
        assert_eq!(
            hex(Sha256::digest(b"")),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            hex(Sha256::digest(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let mut split = Sha256::new();
        split.update(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq".as_slice());
        split.update([]);
        assert_eq!(
            hex(split.finalize()),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }
}
