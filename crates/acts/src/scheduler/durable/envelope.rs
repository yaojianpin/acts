use crate::scheduler::core::commit::aggregate::sha256_hex;
use std::fmt;

const MAGIC: &[u8; 8] = b"ACTSAGR\0";
const HEADER_LEN: usize = 8 + 2 + 8 + 64;
/// The schema the pre-L8 kernel wrote: one whole-snapshot document per
/// process. Reads keep accepting it (and `ProcessDocumentCodec::decode`
/// still decodes it) so an existing database opens unchanged.
pub(crate) const LEGACY_AGGREGATE_SCHEMA_VERSION: u16 = 1;
/// The L8 schema: one head document plus a base snapshot and an append-only
/// log of per-commit records (see `document.rs`). The payload magic
/// distinguishes head/base/log documents inside this envelope.
pub(crate) const AGGREGATE_SCHEMA_VERSION: u16 = 2;

/// Self-validating storage envelope. The payload codec is deliberately kept
/// separate so schema migration and corruption detection cannot be bypassed by
/// repository implementations.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProcessEnvelope {
    pub(crate) schema_version: u16,
    pub(crate) payload_digest: String,
    pub(crate) payload: Vec<u8>,
}

impl ProcessEnvelope {
    /// Seal a legacy (pre-L8) whole-snapshot payload. Test fixtures still
    /// write the old format to exercise the read/migration path.
    #[cfg(test)]
    pub(crate) fn seal(payload: Vec<u8>) -> Result<Vec<u8>, EnvelopeError> {
        Self::seal_with_schema(LEGACY_AGGREGATE_SCHEMA_VERSION, payload)
    }

    /// Seal an L8 document (head, base or log entry).
    ///
    /// The payload is hashed exactly once, here, and the digest goes straight
    /// into the header: the write path must not re-hash the bytes it just
    /// sealed. `decode` still verifies the digest against the payload it
    /// read, so corruption detection at the storage boundary is unchanged.
    pub(crate) fn seal_v2(payload: Vec<u8>) -> Result<Vec<u8>, EnvelopeError> {
        Self::seal_with_schema(AGGREGATE_SCHEMA_VERSION, payload)
    }

    fn seal_with_schema(schema_version: u16, payload: Vec<u8>) -> Result<Vec<u8>, EnvelopeError> {
        let payload_len =
            u64::try_from(payload.len()).map_err(|_| EnvelopeError::PayloadTooLarge)?;
        let mut bytes = Vec::with_capacity(HEADER_LEN + payload.len());
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&schema_version.to_be_bytes());
        bytes.extend_from_slice(&payload_len.to_be_bytes());
        bytes.extend_from_slice(sha256_hex(&payload).as_bytes());
        bytes.extend_from_slice(&payload);
        Ok(bytes)
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, EnvelopeError> {
        if bytes.len() < HEADER_LEN {
            return Err(EnvelopeError::TruncatedHeader);
        }
        if &bytes[..8] != MAGIC {
            return Err(EnvelopeError::InvalidMagic);
        }
        let schema_version = u16::from_be_bytes([bytes[8], bytes[9]]);
        if !matches!(
            schema_version,
            LEGACY_AGGREGATE_SCHEMA_VERSION | AGGREGATE_SCHEMA_VERSION
        ) {
            return Err(EnvelopeError::UnsupportedSchema(schema_version));
        }
        let payload_len = u64::from_be_bytes(
            bytes[10..18]
                .try_into()
                .map_err(|_| EnvelopeError::TruncatedHeader)?,
        );
        let payload_len =
            usize::try_from(payload_len).map_err(|_| EnvelopeError::PayloadTooLarge)?;
        let expected_len = HEADER_LEN
            .checked_add(payload_len)
            .ok_or(EnvelopeError::PayloadTooLarge)?;
        if bytes.len() < expected_len {
            return Err(EnvelopeError::TruncatedPayload);
        }
        if bytes.len() > expected_len {
            return Err(EnvelopeError::TrailingBytes);
        }
        let digest_bytes = &bytes[18..82];
        if !digest_bytes.iter().all(u8::is_ascii_hexdigit) {
            return Err(EnvelopeError::InvalidDigestEncoding);
        }
        let payload_digest = std::str::from_utf8(digest_bytes)
            .map_err(|_| EnvelopeError::InvalidDigestEncoding)?
            .to_ascii_lowercase();
        let payload = bytes[HEADER_LEN..].to_vec();
        let envelope = Self {
            schema_version,
            payload_digest,
            payload,
        };
        envelope.validate()?;
        Ok(envelope)
    }

    pub(crate) fn validate(&self) -> Result<(), EnvelopeError> {
        if !matches!(
            self.schema_version,
            LEGACY_AGGREGATE_SCHEMA_VERSION | AGGREGATE_SCHEMA_VERSION
        ) {
            return Err(EnvelopeError::UnsupportedSchema(self.schema_version));
        }
        if self.payload_digest.len() != 64
            || !self
                .payload_digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(EnvelopeError::InvalidDigestEncoding);
        }
        let actual = sha256_hex(&self.payload);
        if actual != self.payload_digest {
            return Err(EnvelopeError::DigestMismatch {
                expected: self.payload_digest.clone(),
                actual,
            });
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum EnvelopeError {
    InvalidMagic,
    UnsupportedSchema(u16),
    TruncatedHeader,
    TruncatedPayload,
    TrailingBytes,
    PayloadTooLarge,
    InvalidDigestEncoding,
    DigestMismatch { expected: String, actual: String },
}
impl fmt::Display for EnvelopeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "aggregate envelope error: {self:?}")
    }
}
impl std::error::Error for EnvelopeError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_round_trip_is_exact_and_self_validating() {
        let encoded = ProcessEnvelope::seal(b"deterministic payload".to_vec()).unwrap();
        let decoded = ProcessEnvelope::decode(&encoded).unwrap();
        assert_eq!(decoded.payload, b"deterministic payload");
        assert_eq!(decoded.schema_version, LEGACY_AGGREGATE_SCHEMA_VERSION);
        assert_eq!(decoded.payload_digest, sha256_hex(&decoded.payload));
    }

    #[test]
    fn corruption_is_detected_before_payload_decode() {
        let mut encoded = ProcessEnvelope::seal(b"payload".to_vec()).unwrap();
        *encoded.last_mut().unwrap() ^= 0x01;
        assert!(matches!(
            ProcessEnvelope::decode(&encoded),
            Err(EnvelopeError::DigestMismatch { .. })
        ));
    }

    /// The header digest is verified against the payload it describes: a
    /// document whose digest was altered while its payload stayed intact is
    /// rejected too, so validation cannot be skipped by a writer.
    #[test]
    fn tampered_digest_is_rejected_by_decode() {
        let mut encoded = ProcessEnvelope::seal(b"payload".to_vec()).unwrap();
        encoded[18] = if encoded[18] == b'a' { b'b' } else { b'a' };
        assert!(matches!(
            ProcessEnvelope::decode(&encoded),
            Err(EnvelopeError::DigestMismatch { .. })
        ));
    }

    #[test]
    fn unsupported_schema_and_trailing_bytes_are_rejected() {
        let encoded = ProcessEnvelope::seal(Vec::new()).unwrap();
        let mut unsupported = encoded.clone();
        unsupported[8..10].copy_from_slice(&3u16.to_be_bytes());
        assert_eq!(
            ProcessEnvelope::decode(&unsupported),
            Err(EnvelopeError::UnsupportedSchema(3))
        );

        let mut trailing = encoded;
        trailing.push(0);
        assert_eq!(
            ProcessEnvelope::decode(&trailing),
            Err(EnvelopeError::TrailingBytes)
        );
    }

    #[test]
    fn truncation_never_panics_and_is_classified() {
        let encoded = ProcessEnvelope::seal(b"payload".to_vec()).unwrap();
        for length in 0..HEADER_LEN {
            assert_eq!(
                ProcessEnvelope::decode(&encoded[..length]),
                Err(EnvelopeError::TruncatedHeader)
            );
        }
        assert_eq!(
            ProcessEnvelope::decode(&encoded[..HEADER_LEN]),
            Err(EnvelopeError::TruncatedPayload)
        );
    }
}
