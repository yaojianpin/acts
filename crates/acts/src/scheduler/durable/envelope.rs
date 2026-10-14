use crate::scheduler::core::commit::aggregate::sha256_hex;
use std::fmt;

const MAGIC: &[u8; 8] = b"ACTSAGR\0";
const HEADER_LEN: usize = 8 + 2 + 8 + 64;
pub(crate) const AGGREGATE_SCHEMA_VERSION: u16 = 1;

/// Self-validating storage envelope. The payload codec is deliberately kept
/// separate so schema migration and corruption detection cannot be bypassed by
/// repository implementations.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AggregateEnvelope {
    pub(crate) schema_version: u16,
    pub(crate) payload_digest: String,
    pub(crate) payload: Vec<u8>,
}

impl AggregateEnvelope {
    pub(crate) fn seal(payload: Vec<u8>) -> Self {
        Self {
            schema_version: AGGREGATE_SCHEMA_VERSION,
            payload_digest: sha256_hex(&payload),
            payload,
        }
    }

    pub(crate) fn encode(&self) -> Result<Vec<u8>, EnvelopeError> {
        self.validate()?;
        let payload_len =
            u64::try_from(self.payload.len()).map_err(|_| EnvelopeError::PayloadTooLarge)?;
        let mut bytes = Vec::with_capacity(HEADER_LEN + self.payload.len());
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&self.schema_version.to_be_bytes());
        bytes.extend_from_slice(&payload_len.to_be_bytes());
        bytes.extend_from_slice(self.payload_digest.as_bytes());
        bytes.extend_from_slice(&self.payload);
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
        if schema_version != AGGREGATE_SCHEMA_VERSION {
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
        if self.schema_version != AGGREGATE_SCHEMA_VERSION {
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
        let envelope = AggregateEnvelope::seal(b"deterministic payload".to_vec());
        let encoded = envelope.encode().unwrap();
        assert_eq!(AggregateEnvelope::decode(&encoded).unwrap(), envelope);
    }

    #[test]
    fn corruption_is_detected_before_payload_decode() {
        let envelope = AggregateEnvelope::seal(b"payload".to_vec());
        let mut encoded = envelope.encode().unwrap();
        *encoded.last_mut().unwrap() ^= 0x01;
        assert!(matches!(
            AggregateEnvelope::decode(&encoded),
            Err(EnvelopeError::DigestMismatch { .. })
        ));
    }

    #[test]
    fn unsupported_schema_and_trailing_bytes_are_rejected() {
        let envelope = AggregateEnvelope::seal(Vec::new());
        let mut unsupported = envelope.encode().unwrap();
        unsupported[8..10].copy_from_slice(&2u16.to_be_bytes());
        assert_eq!(
            AggregateEnvelope::decode(&unsupported),
            Err(EnvelopeError::UnsupportedSchema(2))
        );

        let mut trailing = envelope.encode().unwrap();
        trailing.push(0);
        assert_eq!(
            AggregateEnvelope::decode(&trailing),
            Err(EnvelopeError::TrailingBytes)
        );
    }

    #[test]
    fn truncation_never_panics_and_is_classified() {
        let encoded = AggregateEnvelope::seal(b"payload".to_vec())
            .encode()
            .unwrap();
        for length in 0..HEADER_LEN {
            assert_eq!(
                AggregateEnvelope::decode(&encoded[..length]),
                Err(EnvelopeError::TruncatedHeader)
            );
        }
        assert_eq!(
            AggregateEnvelope::decode(&encoded[..HEADER_LEN]),
            Err(EnvelopeError::TruncatedPayload)
        );
    }
}
