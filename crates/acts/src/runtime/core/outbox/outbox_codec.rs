use crate::runtime::core::outbox::records::{
    DeliveryClaim, OutboxError, OutboxId, OutboxRecord, OutboxState,
};
use std::fmt;

const MAGIC: &[u8; 8] = b"ACTOBX\0\0";
const SCHEMA_V1: u16 = 1;

pub(crate) struct OutboxCodec;

impl OutboxCodec {
    pub(crate) fn encode(record: &OutboxRecord) -> Result<Vec<u8>, OutboxCodecError> {
        record.validate().map_err(OutboxCodecError::InvalidRecord)?;
        let mut writer = Writer::default();
        writer.raw(MAGIC);
        writer.u16(SCHEMA_V1);
        writer.string(&record.id.process_id)?;
        writer.string(&record.id.key)?;
        writer.string(&record.topic)?;
        writer.string(&record.payload)?;
        writer.u32(record.attempt);
        writer.u64(record.last_fence);
        writer.u64(record.revision);
        match &record.state {
            OutboxState::Pending { available_at_ms } => {
                writer.u8(0);
                writer.i64(*available_at_ms);
            }
            OutboxState::Claimed(claim) => {
                writer.u8(1);
                writer.claim(claim)?;
            }
            OutboxState::DeliveryUnknown { reason } => {
                writer.u8(2);
                writer.string(reason)?;
            }
            OutboxState::Delivered { receipt } => {
                writer.u8(3);
                writer.string(receipt)?;
            }
            OutboxState::Quarantined { reason } => {
                writer.u8(4);
                writer.string(reason)?;
            }
        }
        Ok(writer.bytes)
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<OutboxRecord, OutboxCodecError> {
        let mut reader = Reader { bytes, offset: 0 };
        if reader.take(MAGIC.len())? != MAGIC {
            return Err(OutboxCodecError::InvalidMagic);
        }
        let schema = reader.u16()?;
        if schema != SCHEMA_V1 {
            return Err(OutboxCodecError::UnsupportedSchema(schema));
        }
        let id = OutboxId {
            process_id: reader.string()?,
            key: reader.string()?,
        };
        let topic = reader.string()?;
        let payload = reader.string()?;
        let attempt = reader.u32()?;
        let last_fence = reader.u64()?;
        let revision = reader.u64()?;
        let state = match reader.u8()? {
            0 => OutboxState::Pending {
                available_at_ms: reader.i64()?,
            },
            1 => OutboxState::Claimed(reader.claim()?),
            2 => OutboxState::DeliveryUnknown {
                reason: reader.string()?,
            },
            3 => OutboxState::Delivered {
                receipt: reader.string()?,
            },
            4 => OutboxState::Quarantined {
                reason: reader.string()?,
            },
            tag => return Err(OutboxCodecError::UnknownState(tag)),
        };
        if reader.offset != bytes.len() {
            return Err(OutboxCodecError::TrailingBytes);
        }
        let record = OutboxRecord {
            id,
            topic,
            payload,
            state,
            attempt,
            last_fence,
            revision,
        };
        record.validate().map_err(OutboxCodecError::InvalidRecord)?;
        Ok(record)
    }
}

#[derive(Default)]
struct Writer {
    bytes: Vec<u8>,
}
impl Writer {
    fn raw(&mut self, value: &[u8]) {
        self.bytes.extend_from_slice(value);
    }
    fn u8(&mut self, value: u8) {
        self.bytes.push(value);
    }
    fn u16(&mut self, value: u16) {
        self.raw(&value.to_be_bytes());
    }
    fn u32(&mut self, value: u32) {
        self.raw(&value.to_be_bytes());
    }
    fn u64(&mut self, value: u64) {
        self.raw(&value.to_be_bytes());
    }
    fn i64(&mut self, value: i64) {
        self.raw(&value.to_be_bytes());
    }
    fn string(&mut self, value: &str) -> Result<(), OutboxCodecError> {
        self.u32(u32::try_from(value.len()).map_err(|_| OutboxCodecError::StringTooLarge)?);
        self.raw(value.as_bytes());
        Ok(())
    }
    fn claim(&mut self, claim: &DeliveryClaim) -> Result<(), OutboxCodecError> {
        self.string(&claim.owner)?;
        self.u64(claim.fence);
        self.i64(claim.lease_until_ms);
        self.u64(claim.heartbeat_revision);
        Ok(())
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}
impl<'a> Reader<'a> {
    fn take(&mut self, length: usize) -> Result<&'a [u8], OutboxCodecError> {
        let end = self
            .offset
            .checked_add(length)
            .ok_or(OutboxCodecError::Truncated)?;
        let value = self
            .bytes
            .get(self.offset..end)
            .ok_or(OutboxCodecError::Truncated)?;
        self.offset = end;
        Ok(value)
    }
    fn u8(&mut self) -> Result<u8, OutboxCodecError> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, OutboxCodecError> {
        let bytes = self.take(2)?;
        let mut array = [0_u8; 2];
        array.copy_from_slice(bytes);
        Ok(u16::from_be_bytes(array))
    }
    fn u32(&mut self) -> Result<u32, OutboxCodecError> {
        let bytes = self.take(4)?;
        let mut array = [0_u8; 4];
        array.copy_from_slice(bytes);
        Ok(u32::from_be_bytes(array))
    }
    fn u64(&mut self) -> Result<u64, OutboxCodecError> {
        let bytes = self.take(8)?;
        let mut array = [0_u8; 8];
        array.copy_from_slice(bytes);
        Ok(u64::from_be_bytes(array))
    }
    fn i64(&mut self) -> Result<i64, OutboxCodecError> {
        let bytes = self.take(8)?;
        let mut array = [0_u8; 8];
        array.copy_from_slice(bytes);
        Ok(i64::from_be_bytes(array))
    }
    fn string(&mut self) -> Result<String, OutboxCodecError> {
        let length = usize::try_from(self.u32()?).map_err(|_| OutboxCodecError::StringTooLarge)?;
        String::from_utf8(self.take(length)?.to_vec()).map_err(|_| OutboxCodecError::InvalidUtf8)
    }
    fn claim(&mut self) -> Result<DeliveryClaim, OutboxCodecError> {
        Ok(DeliveryClaim {
            owner: self.string()?,
            fence: self.u64()?,
            lease_until_ms: self.i64()?,
            heartbeat_revision: self.u64()?,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum OutboxCodecError {
    InvalidMagic,
    UnsupportedSchema(u16),
    UnknownState(u8),
    Truncated,
    TrailingBytes,
    InvalidUtf8,
    StringTooLarge,
    InvalidRecord(OutboxError),
}
impl fmt::Display for OutboxCodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "outbox codec error: {self:?}")
    }
}
impl std::error::Error for OutboxCodecError {}

#[cfg(test)]
mod tests {
    use super::*;
    fn pending() -> OutboxRecord {
        OutboxRecord::pending(
            OutboxId {
                process_id: "p".into(),
                key: "k".into(),
            },
            "topic".into(),
            "payload".into(),
            -1,
        )
        .unwrap()
    }
    #[test]
    fn codec_round_trips_pending_and_claimed_records() {
        let pending = pending();
        let bytes = OutboxCodec::encode(&pending).unwrap();
        assert_eq!(OutboxCodec::decode(&bytes).unwrap(), pending);

        let claim = DeliveryClaim {
            owner: "worker".into(),
            fence: 1,
            lease_until_ms: 50,
            heartbeat_revision: 0,
        };
        let claimed = pending
            .evolve(crate::runtime::core::outbox::records::OutboxEvent::Claim { claim, now_ms: 0 })
            .unwrap();
        let bytes = OutboxCodec::encode(&claimed).unwrap();
        assert_eq!(OutboxCodec::decode(&bytes).unwrap(), claimed);
    }
    #[test]
    fn decoder_rejects_all_truncated_prefixes_and_trailing_bytes() {
        let mut bytes = OutboxCodec::encode(&pending()).unwrap();
        for length in 0..bytes.len() {
            assert_eq!(
                OutboxCodec::decode(&bytes[..length]),
                Err(OutboxCodecError::Truncated)
            );
        }
        bytes.push(0);
        assert_eq!(
            OutboxCodec::decode(&bytes),
            Err(OutboxCodecError::TrailingBytes)
        );
    }
    #[test]
    fn decoder_revalidates_counter_invariants() {
        let mut bytes = OutboxCodec::encode(&pending()).unwrap();
        let attempt_offset = 8 + 2 + 4 + 1 + 4 + 1 + 4 + 5 + 4 + 7;
        bytes[attempt_offset + 3] = 1;
        assert!(matches!(
            OutboxCodec::decode(&bytes),
            Err(OutboxCodecError::InvalidRecord(
                OutboxError::CounterInvariant
            ))
        ));
    }
}
