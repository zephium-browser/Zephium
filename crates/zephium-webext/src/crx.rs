//! CRX3 package verification.

use ring::signature::{self, UnparsedPublicKey, VerificationAlgorithm};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::ExtensionId;

pub const MAX_CRX_SIZE: usize = 256 * 1024 * 1024;
pub const MAX_HEADER_SIZE: usize = 256 * 1024;

const MAGIC: &[u8; 4] = b"Cr24";
const SIGNATURE_CONTEXT: &[u8] = b"CRX3 SignedData\x00";
const ZIP_MAGIC: &[u8; 4] = b"PK\x03\x04";

const OID_RSA_ENCRYPTION: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01];
const OID_EC_PUBLIC_KEY: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01];
const OID_PRIME256V1: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07];

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum CrxError {
    #[error("package exceeds the {MAX_CRX_SIZE}-byte limit")]
    TooLarge,
    #[error("package is truncated")]
    Truncated,
    #[error("not a CRX package")]
    Magic,
    #[error("unsupported CRX version {0}")]
    Version(u32),
    #[error("malformed CRX header: {0}")]
    Header(&'static str),
    #[error("CRX payload is not a ZIP archive")]
    Archive,
    #[error("unsupported or malformed public key")]
    Key,
    #[error("no signature proof matches the declared extension ID")]
    NoDeveloperProof,
    #[error("signature verification failed")]
    BadSignature,
    #[error("package is extension {actual}, expected {expected}")]
    IdMismatch {
        expected: ExtensionId,
        actual: ExtensionId,
    },
}

#[derive(Debug)]
pub struct VerifiedCrx<'a> {
    pub id: ExtensionId,
    /// Developer key as SubjectPublicKeyInfo DER.
    pub public_key: &'a [u8],
    pub zip: &'a [u8],
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum KeyKind {
    Rsa,
    Ecdsa,
}

struct Proof<'a> {
    kind: KeyKind,
    public_key: &'a [u8],
    signature: &'a [u8],
}

/// Verifies a CRX3 package with Chrome's semantics: every proof must verify,
/// and one of them must be made by the key that derives the declared ID.
pub fn verify<'a>(
    bytes: &'a [u8],
    expected: Option<&ExtensionId>,
) -> Result<VerifiedCrx<'a>, CrxError> {
    if bytes.len() > MAX_CRX_SIZE {
        return Err(CrxError::TooLarge);
    }
    let preamble = bytes.get(..12).ok_or(CrxError::Truncated)?;
    if &preamble[..4] != MAGIC {
        return Err(CrxError::Magic);
    }
    let version = u32::from_le_bytes(preamble[4..8].try_into().unwrap());
    if version != 3 {
        return Err(CrxError::Version(version));
    }
    let header_len = u32::from_le_bytes(preamble[8..12].try_into().unwrap()) as usize;
    if header_len > MAX_HEADER_SIZE {
        return Err(CrxError::Header("header exceeds size limit"));
    }
    let header = bytes.get(12..12 + header_len).ok_or(CrxError::Truncated)?;
    let zip = &bytes[12 + header_len..];
    if !zip.starts_with(ZIP_MAGIC) {
        return Err(CrxError::Archive);
    }

    let (proofs, signed_header_data) = parse_header(header)?;
    let crx_id = parse_signed_data(signed_header_data)?;

    let developer = proofs
        .iter()
        .find(|proof| Sha256::digest(proof.public_key)[..16] == crx_id)
        .ok_or(CrxError::NoDeveloperProof)?;

    let signed_len = u32::try_from(signed_header_data.len())
        .map_err(|_| CrxError::Header("signed data too large"))?;
    let mut message =
        Vec::with_capacity(SIGNATURE_CONTEXT.len() + 4 + signed_header_data.len() + zip.len());
    message.extend_from_slice(SIGNATURE_CONTEXT);
    message.extend_from_slice(&signed_len.to_le_bytes());
    message.extend_from_slice(signed_header_data);
    message.extend_from_slice(zip);

    for proof in &proofs {
        let key = subject_public_key(proof.public_key, proof.kind)?;
        let algorithm: &dyn VerificationAlgorithm = match proof.kind {
            KeyKind::Rsa => &signature::RSA_PKCS1_1024_8192_SHA256_FOR_LEGACY_USE_ONLY,
            KeyKind::Ecdsa => &signature::ECDSA_P256_SHA256_ASN1,
        };
        UnparsedPublicKey::new(algorithm, key)
            .verify(&message, proof.signature)
            .map_err(|_| CrxError::BadSignature)?;
    }

    let id = ExtensionId::from_hash_prefix(&crx_id);
    if let Some(expected) = expected {
        if *expected != id {
            return Err(CrxError::IdMismatch {
                expected: expected.clone(),
                actual: id,
            });
        }
    }
    Ok(VerifiedCrx {
        id,
        public_key: developer.public_key,
        zip,
    })
}

fn parse_header(header: &[u8]) -> Result<(Vec<Proof<'_>>, &[u8]), CrxError> {
    let mut proofs = Vec::new();
    let mut signed_header_data = None;
    let mut reader = ProtoReader::new(header);
    while let Some((field, value)) = reader.next_field()? {
        match (field, value) {
            (2, Some(bytes)) => proofs.push(parse_proof(bytes, KeyKind::Rsa)?),
            (3, Some(bytes)) => proofs.push(parse_proof(bytes, KeyKind::Ecdsa)?),
            (10000, Some(bytes)) => signed_header_data = Some(bytes),
            (2 | 3 | 10000, None) => return Err(CrxError::Header("unexpected wire type")),
            _ => {}
        }
    }
    let signed_header_data = signed_header_data.ok_or(CrxError::Header("missing signed data"))?;
    Ok((proofs, signed_header_data))
}

fn parse_proof(bytes: &[u8], kind: KeyKind) -> Result<Proof<'_>, CrxError> {
    let (mut public_key, mut signature) = (None, None);
    let mut reader = ProtoReader::new(bytes);
    while let Some((field, value)) = reader.next_field()? {
        match (field, value) {
            (1, Some(bytes)) => public_key = Some(bytes),
            (2, Some(bytes)) => signature = Some(bytes),
            (1 | 2, None) => return Err(CrxError::Header("unexpected wire type")),
            _ => {}
        }
    }
    match (public_key, signature) {
        (Some(public_key), Some(signature)) => Ok(Proof {
            kind,
            public_key,
            signature,
        }),
        _ => Err(CrxError::Header("incomplete key proof")),
    }
}

fn parse_signed_data(bytes: &[u8]) -> Result<[u8; 16], CrxError> {
    let mut crx_id = None;
    let mut reader = ProtoReader::new(bytes);
    while let Some((field, value)) = reader.next_field()? {
        if field == 1 {
            crx_id = Some(value.ok_or(CrxError::Header("unexpected wire type"))?);
        }
    }
    crx_id
        .and_then(|id| id.try_into().ok())
        .ok_or(CrxError::Header("missing or malformed crx_id"))
}

/// A bounded protobuf reader that yields length-delimited payloads and skips
/// scalar fields. Groups (wire types 3 and 4) are rejected.
struct ProtoReader<'a> {
    buf: &'a [u8],
    pos: usize,
}

/// A field number and, for length-delimited fields, its payload.
type Field<'a> = (u64, Option<&'a [u8]>);

impl<'a> ProtoReader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn next_field(&mut self) -> Result<Option<Field<'a>>, CrxError> {
        if self.pos == self.buf.len() {
            return Ok(None);
        }
        let tag = self.varint()?;
        let field = tag >> 3;
        if field == 0 || field > u64::from(u32::MAX >> 3) {
            return Err(CrxError::Header("invalid field number"));
        }
        let value = match tag & 7 {
            0 => {
                self.varint()?;
                None
            }
            1 => {
                self.take(8)?;
                None
            }
            2 => {
                let len = usize::try_from(self.varint()?)
                    .map_err(|_| CrxError::Header("length overflow"))?;
                Some(self.take(len)?)
            }
            5 => {
                self.take(4)?;
                None
            }
            _ => return Err(CrxError::Header("unsupported wire type")),
        };
        Ok(Some((field, value)))
    }

    fn varint(&mut self) -> Result<u64, CrxError> {
        let mut value = 0u64;
        for i in 0..10 {
            let byte = *self
                .buf
                .get(self.pos)
                .ok_or(CrxError::Header("truncated varint"))?;
            self.pos += 1;
            if i == 9 && byte > 1 {
                return Err(CrxError::Header("varint overflow"));
            }
            value |= u64::from(byte & 0x7f) << (7 * i);
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(CrxError::Header("varint overflow"))
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], CrxError> {
        let end = self
            .pos
            .checked_add(len)
            .filter(|&end| end <= self.buf.len())
            .ok_or(CrxError::Header("truncated field"))?;
        let bytes = &self.buf[self.pos..end];
        self.pos = end;
        Ok(bytes)
    }
}

/// Returns the key bytes ring expects from a SubjectPublicKeyInfo: the
/// PKCS#1 RSAPublicKey for RSA, or the uncompressed point for P-256.
fn subject_public_key(spki: &[u8], kind: KeyKind) -> Result<&[u8], CrxError> {
    let mut outer = Der::new(spki);
    let mut info = Der::new(outer.expect(0x30)?);
    outer.finish()?;
    let mut algorithm = Der::new(info.expect(0x30)?);
    let bits = info.expect(0x03)?;
    info.finish()?;

    let oid = algorithm.expect(0x06)?;
    match kind {
        KeyKind::Rsa if oid == OID_RSA_ENCRYPTION => {
            if !algorithm.is_empty() {
                algorithm.expect(0x05)?;
            }
        }
        KeyKind::Ecdsa if oid == OID_EC_PUBLIC_KEY => {
            if algorithm.expect(0x06)? != OID_PRIME256V1 {
                return Err(CrxError::Key);
            }
        }
        _ => return Err(CrxError::Key),
    }
    algorithm.finish()?;

    match bits.split_first() {
        Some((0, key)) if !key.is_empty() => Ok(key),
        _ => Err(CrxError::Key),
    }
}

struct Der<'a> {
    buf: &'a [u8],
}

impl<'a> Der<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf }
    }

    fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    fn expect(&mut self, tag: u8) -> Result<&'a [u8], CrxError> {
        let (&actual, rest) = self.buf.split_first().ok_or(CrxError::Key)?;
        if actual != tag {
            return Err(CrxError::Key);
        }
        let (&first, rest) = rest.split_first().ok_or(CrxError::Key)?;
        let (len, rest) = match first {
            0..=0x7f => (usize::from(first), rest),
            0x81 => match rest.split_first() {
                Some((&len, rest)) if len >= 0x80 => (usize::from(len), rest),
                _ => return Err(CrxError::Key),
            },
            0x82 => match rest {
                [hi, lo, rest @ ..] if *hi != 0 => (usize::from(*hi) << 8 | usize::from(*lo), rest),
                _ => return Err(CrxError::Key),
            },
            _ => return Err(CrxError::Key),
        };
        if rest.len() < len {
            return Err(CrxError::Key);
        }
        let (contents, rest) = rest.split_at(len);
        self.buf = rest;
        Ok(contents)
    }

    fn finish(&self) -> Result<(), CrxError> {
        if self.buf.is_empty() {
            Ok(())
        } else {
            Err(CrxError::Key)
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::io::{Cursor, Write};

    use ring::rand::SystemRandom;
    use ring::signature::{EcdsaKeyPair, KeyPair, ECDSA_P256_SHA256_ASN1_SIGNING};
    use zip::write::SimpleFileOptions;

    use super::*;

    const P256_SPKI_PREFIX: &[u8] = &[
        0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08,
        0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00,
    ];

    pub(crate) struct TestKey {
        pair: EcdsaKeyPair,
        pub(crate) spki: Vec<u8>,
    }

    impl TestKey {
        pub(crate) fn generate() -> Self {
            let rng = SystemRandom::new();
            let pkcs8 =
                EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &rng).unwrap();
            let pair =
                EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, pkcs8.as_ref(), &rng)
                    .unwrap();
            let spki = [P256_SPKI_PREFIX, pair.public_key().as_ref()].concat();
            Self { pair, spki }
        }

        pub(crate) fn id(&self) -> ExtensionId {
            ExtensionId::from_public_key(&self.spki)
        }

        fn sign(&self, message: &[u8]) -> Vec<u8> {
            self.pair
                .sign(&SystemRandom::new(), message)
                .unwrap()
                .as_ref()
                .to_vec()
        }
    }

    pub(crate) fn zip_of(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, contents) in files {
            writer
                .start_file(*name, SimpleFileOptions::default())
                .unwrap();
            writer.write_all(contents).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    fn bytes_field(out: &mut Vec<u8>, field: u64, bytes: &[u8]) {
        varint(out, field << 3 | 2);
        varint(out, bytes.len() as u64);
        out.extend_from_slice(bytes);
    }

    fn varint(out: &mut Vec<u8>, mut value: u64) {
        while value >= 0x80 {
            out.push(value as u8 | 0x80);
            value >>= 7;
        }
        out.push(value as u8);
    }

    fn signed_message(signed_header_data: &[u8], zip: &[u8]) -> Vec<u8> {
        let mut message = SIGNATURE_CONTEXT.to_vec();
        message.extend_from_slice(&(signed_header_data.len() as u32).to_le_bytes());
        message.extend_from_slice(signed_header_data);
        message.extend_from_slice(zip);
        message
    }

    /// Builds a CRX3 whose declared ID is `declared`'s, signed by `signers`.
    pub(crate) fn build_crx(declared: &TestKey, signers: &[&TestKey], zip: &[u8]) -> Vec<u8> {
        build_crx_with_forgeries(declared, signers, &[], zip)
    }

    fn build_crx_with_forgeries(
        declared: &TestKey,
        signers: &[&TestKey],
        forgers: &[&TestKey],
        zip: &[u8],
    ) -> Vec<u8> {
        let mut signed_data = Vec::new();
        bytes_field(&mut signed_data, 1, &Sha256::digest(&declared.spki)[..16]);
        let message = signed_message(&signed_data, zip);

        let mut header = Vec::new();
        for key in signers {
            let mut proof = Vec::new();
            bytes_field(&mut proof, 1, &key.spki);
            bytes_field(&mut proof, 2, &key.sign(&message));
            bytes_field(&mut header, 3, &proof);
        }
        for key in forgers {
            let mut proof = Vec::new();
            bytes_field(&mut proof, 1, &key.spki);
            bytes_field(&mut proof, 2, &key.sign(b"something else"));
            bytes_field(&mut header, 3, &proof);
        }
        varint(&mut header, 7 << 3);
        varint(&mut header, 300);
        bytes_field(&mut header, 10000, &signed_data);

        let mut crx = MAGIC.to_vec();
        crx.extend_from_slice(&3u32.to_le_bytes());
        crx.extend_from_slice(&(header.len() as u32).to_le_bytes());
        crx.extend_from_slice(&header);
        crx.extend_from_slice(zip);
        crx
    }

    #[test]
    fn verifies_a_signed_package() {
        let key = TestKey::generate();
        let zip = zip_of(&[("manifest.json", b"{}")]);
        let crx = build_crx(&key, &[&key], &zip);
        let verified = verify(&crx, Some(&key.id())).unwrap();
        assert_eq!(verified.id, key.id());
        assert_eq!(verified.public_key, key.spki.as_slice());
        assert_eq!(verified.zip, zip.as_slice());
    }

    #[test]
    fn requires_every_proof_to_verify() {
        let developer = TestKey::generate();
        let other = TestKey::generate();
        let zip = zip_of(&[("manifest.json", b"{}")]);
        let crx = build_crx(&developer, &[&other, &developer], &zip);
        assert_eq!(verify(&crx, None).unwrap().id, developer.id());

        let forged = build_crx_with_forgeries(&developer, &[&developer], &[&other], &zip);
        assert_eq!(verify(&forged, None).unwrap_err(), CrxError::BadSignature);
    }

    #[test]
    fn rejects_tampered_archive() {
        let key = TestKey::generate();
        let mut crx = build_crx(&key, &[&key], &zip_of(&[("manifest.json", b"{}")]));
        let last = crx.len() - 1;
        crx[last] ^= 0xff;
        assert_eq!(verify(&crx, None).unwrap_err(), CrxError::BadSignature);
    }

    #[test]
    fn rejects_unexpected_id() {
        let key = TestKey::generate();
        let crx = build_crx(&key, &[&key], &zip_of(&[("manifest.json", b"{}")]));
        let expected = TestKey::generate().id();
        assert!(matches!(
            verify(&crx, Some(&expected)),
            Err(CrxError::IdMismatch { actual, .. }) if actual == key.id()
        ));
    }

    #[test]
    fn requires_a_developer_proof() {
        let claimed = TestKey::generate();
        let signer = TestKey::generate();
        let crx = build_crx(&claimed, &[&signer], &zip_of(&[("manifest.json", b"{}")]));
        assert_eq!(verify(&crx, None).unwrap_err(), CrxError::NoDeveloperProof);
        let unsigned = build_crx(&claimed, &[], &zip_of(&[("manifest.json", b"{}")]));
        assert_eq!(
            verify(&unsigned, None).unwrap_err(),
            CrxError::NoDeveloperProof
        );
    }

    #[test]
    fn rejects_truncated_and_malformed_input() {
        let key = TestKey::generate();
        let crx = build_crx(&key, &[&key], &zip_of(&[("manifest.json", b"{}")]));
        assert_eq!(verify(&crx[..8], None).unwrap_err(), CrxError::Truncated);
        assert_eq!(verify(&crx[..40], None).unwrap_err(), CrxError::Truncated);

        let header_len = u32::from_le_bytes(crx[8..12].try_into().unwrap()) as usize;
        assert!(verify(&crx[..12 + header_len + 10], None).is_err());

        let mut bad_magic = crx.clone();
        bad_magic[0] = b'X';
        assert_eq!(verify(&bad_magic, None).unwrap_err(), CrxError::Magic);

        let mut v2 = crx.clone();
        v2[4] = 2;
        assert_eq!(verify(&v2, None).unwrap_err(), CrxError::Version(2));

        let mut not_zip = crx.clone();
        not_zip[12 + header_len] = b'Q';
        assert_eq!(verify(&not_zip, None).unwrap_err(), CrxError::Archive);

        let mut huge_header = crx;
        huge_header[8..12].copy_from_slice(&(MAX_HEADER_SIZE as u32 + 1).to_le_bytes());
        assert!(matches!(
            verify(&huge_header, None),
            Err(CrxError::Header(_))
        ));
    }

    #[test]
    fn proto_reader_rejects_malformed_varints_and_lengths() {
        let overlong = [0xffu8; 11];
        assert!(ProtoReader::new(&overlong).next_field().is_err());
        let truncated_len = [0x12, 0x05, 0x00];
        assert!(ProtoReader::new(&truncated_len).next_field().is_err());
        let group = [0x13];
        assert!(ProtoReader::new(&group).next_field().is_err());
        let fixed = [0x09, 0, 0, 0, 0, 0, 0, 0, 0, 0x15, 0, 0, 0, 0];
        let mut reader = ProtoReader::new(&fixed);
        assert_eq!(reader.next_field().unwrap(), Some((1, None)));
        assert_eq!(reader.next_field().unwrap(), Some((2, None)));
        assert_eq!(reader.next_field().unwrap(), None);
    }

    #[test]
    fn extracts_subject_public_key_by_algorithm() {
        let key = TestKey::generate();
        let point = subject_public_key(&key.spki, KeyKind::Ecdsa).unwrap();
        assert_eq!(point.len(), 65);
        assert_eq!(point[0], 0x04);
        assert_eq!(
            subject_public_key(&key.spki, KeyKind::Rsa).unwrap_err(),
            CrxError::Key
        );
        let mut trailing = key.spki.clone();
        trailing.push(0);
        assert_eq!(
            subject_public_key(&trailing, KeyKind::Ecdsa).unwrap_err(),
            CrxError::Key
        );
    }
}
