use std::fmt;

pub(super) const RECOVERY_FORMAT_KEY: &str = "kernel/meta/recovery-index-format";
const MAGIC: &[u8; 8] = b"ACTRFMT\0";
const SCHEMA_V1: u16 = 1;
const SCHEMA_V2: u16 = 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RecoveryFormatPhase {
    V1,
    Migrating,
    V2,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct RecoveryFormatDocument {
    pub(super) phase: RecoveryFormatPhase,
    /// Last fully rebuilt V1 process-index key. It advances atomically with the
    /// V2 entries generated from that process, so migration can resume after a
    /// crash without guessing which mutations became visible.
    pub(super) cursor: Option<String>,
}

impl RecoveryFormatDocument {
    pub(super) fn v1() -> Self {
        Self {
            phase: RecoveryFormatPhase::V1,
            cursor: None,
        }
    }

    pub(super) fn migrating(cursor: Option<String>) -> Self {
        Self {
            phase: RecoveryFormatPhase::Migrating,
            cursor,
        }
    }

    pub(super) fn v2() -> Self {
        Self {
            phase: RecoveryFormatPhase::V2,
            cursor: None,
        }
    }

    pub(super) fn encode(&self) -> Result<Vec<u8>, RecoveryFormatError> {
        if self.phase != RecoveryFormatPhase::Migrating && self.cursor.is_some() {
            return Err(RecoveryFormatError::InvalidCursor);
        }
        let cursor = self.cursor.as_deref().unwrap_or_default().as_bytes();
        let cursor_len =
            u32::try_from(cursor.len()).map_err(|_| RecoveryFormatError::CursorTooLarge)?;
        let mut bytes = Vec::with_capacity(MAGIC.len() + 7 + cursor.len());
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&SCHEMA_V2.to_be_bytes());
        bytes.push(phase_tag(self.phase));
        bytes.extend_from_slice(&cursor_len.to_be_bytes());
        bytes.extend_from_slice(cursor);
        Ok(bytes)
    }

    pub(super) fn decode(bytes: &[u8]) -> Result<Self, RecoveryFormatError> {
        if bytes.len() < MAGIC.len() + 3 || &bytes[..MAGIC.len()] != MAGIC {
            return Err(RecoveryFormatError::InvalidDocument);
        }
        let mut schema = [0_u8; 2];
        schema.copy_from_slice(&bytes[MAGIC.len()..MAGIC.len() + 2]);
        match u16::from_be_bytes(schema) {
            SCHEMA_V1 => decode_legacy(bytes),
            SCHEMA_V2 => decode_current(bytes),
            _ => Err(RecoveryFormatError::UnsupportedSchema),
        }
    }
}

fn decode_legacy(bytes: &[u8]) -> Result<RecoveryFormatDocument, RecoveryFormatError> {
    if bytes.len() != MAGIC.len() + 3 {
        return Err(RecoveryFormatError::InvalidDocument);
    }
    Ok(RecoveryFormatDocument {
        phase: decode_phase(bytes[MAGIC.len() + 2])?,
        cursor: None,
    })
}

fn decode_current(bytes: &[u8]) -> Result<RecoveryFormatDocument, RecoveryFormatError> {
    const HEADER_AFTER_MAGIC: usize = 2 + 1 + 4;
    if bytes.len() < MAGIC.len() + HEADER_AFTER_MAGIC {
        return Err(RecoveryFormatError::InvalidDocument);
    }
    let phase = decode_phase(bytes[MAGIC.len() + 2])?;
    let length_offset = MAGIC.len() + 3;
    let mut length_bytes = [0_u8; 4];
    length_bytes.copy_from_slice(&bytes[length_offset..length_offset + 4]);
    let length = usize::try_from(u32::from_be_bytes(length_bytes))
        .map_err(|_| RecoveryFormatError::CursorTooLarge)?;
    let cursor_offset = length_offset + 4;
    let cursor_end = cursor_offset
        .checked_add(length)
        .ok_or(RecoveryFormatError::CursorTooLarge)?;
    if cursor_end != bytes.len() {
        return Err(RecoveryFormatError::InvalidDocument);
    }
    let cursor = if length == 0 {
        None
    } else {
        Some(
            std::str::from_utf8(&bytes[cursor_offset..cursor_end])
                .map_err(|_| RecoveryFormatError::InvalidCursor)?
                .to_owned(),
        )
    };
    if phase != RecoveryFormatPhase::Migrating && cursor.is_some() {
        return Err(RecoveryFormatError::InvalidCursor);
    }
    Ok(RecoveryFormatDocument { phase, cursor })
}

fn phase_tag(phase: RecoveryFormatPhase) -> u8 {
    match phase {
        RecoveryFormatPhase::V1 => 1,
        RecoveryFormatPhase::Migrating => 2,
        RecoveryFormatPhase::V2 => 3,
    }
}

fn decode_phase(value: u8) -> Result<RecoveryFormatPhase, RecoveryFormatError> {
    match value {
        1 => Ok(RecoveryFormatPhase::V1),
        2 => Ok(RecoveryFormatPhase::Migrating),
        3 => Ok(RecoveryFormatPhase::V2),
        _ => Err(RecoveryFormatError::InvalidPhase),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RecoveryFormatError {
    InvalidDocument,
    UnsupportedSchema,
    InvalidPhase,
    InvalidCursor,
    CursorTooLarge,
}

impl fmt::Display for RecoveryFormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDocument => write!(f, "invalid recovery format document"),
            Self::UnsupportedSchema => write!(f, "unsupported recovery format schema"),
            Self::InvalidPhase => write!(f, "invalid recovery format phase"),
            Self::InvalidCursor => write!(f, "invalid recovery migration cursor"),
            Self::CursorTooLarge => write!(f, "recovery migration cursor is too large"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phases_and_migration_cursor_round_trip() {
        for document in [
            RecoveryFormatDocument::v1(),
            RecoveryFormatDocument::migrating(None),
            RecoveryFormatDocument::migrating(Some("kernel/indexes/recovery/70".into())),
            RecoveryFormatDocument::v2(),
        ] {
            assert_eq!(
                RecoveryFormatDocument::decode(&document.encode().unwrap()),
                Ok(document)
            );
        }
    }

    #[test]
    fn legacy_phase_document_remains_readable() {
        let mut legacy = Vec::new();
        legacy.extend_from_slice(MAGIC);
        legacy.extend_from_slice(&SCHEMA_V1.to_be_bytes());
        legacy.push(2);
        assert_eq!(
            RecoveryFormatDocument::decode(&legacy),
            Ok(RecoveryFormatDocument::migrating(None))
        );
    }

    #[test]
    fn damaged_documents_are_rejected() {
        assert_eq!(
            RecoveryFormatDocument::decode(b"short"),
            Err(RecoveryFormatError::InvalidDocument)
        );
        let mut invalid_phase = RecoveryFormatDocument::v1().encode().unwrap();
        invalid_phase[MAGIC.len() + 2] = 9;
        assert_eq!(
            RecoveryFormatDocument::decode(&invalid_phase),
            Err(RecoveryFormatError::InvalidPhase)
        );
        let mut unsupported = RecoveryFormatDocument::v1().encode().unwrap();
        unsupported[MAGIC.len() + 1] = 9;
        assert_eq!(
            RecoveryFormatDocument::decode(&unsupported),
            Err(RecoveryFormatError::UnsupportedSchema)
        );
    }

    #[test]
    fn cursor_is_only_valid_while_migrating() {
        let invalid = RecoveryFormatDocument {
            phase: RecoveryFormatPhase::V2,
            cursor: Some("cursor".into()),
        };
        assert_eq!(invalid.encode(), Err(RecoveryFormatError::InvalidCursor));
    }
}
