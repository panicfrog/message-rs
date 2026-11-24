use thiserror::Error;

#[derive(Debug, Error)]
pub enum SchemaError {
    #[error("message key {0} is duplicated (keys must be globally unique and non-zero)")]
    DuplicateKey(u32),
    #[error("name `{0}` is defined more than once")]
    DuplicateName(String),
    #[error("unknown struct `{0}`: referenced before definition or not defined in this schema set")]
    UnknownType(String),
    #[error("field `{0}`: invalid array/map element type (map cannot be an element in V1)")]
    InvalidElement(String),
    #[error("invalid map key type `{0}` (key must be a fixed-size scalar, enum, or string)")]
    InvalidMapKey(String),
    #[error("field `{0}`: since must be >= {1}")]
    InvalidSince(String, u16),
    #[error("message key must be non-zero")]
    ZeroKey,
}

/// Strict check of an untrusted buffer (`Xxx::validate`). Leaves describe what is wrong; the
/// generated code wraps them in [`ValidationError::Nested`] down to the field (and element) where
/// the problem was found, e.g. `at `edges`[0]: at `children`[1]: invalid utf-8`.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ValidationError {
    #[error("buffer too short: need {need} bytes, got {got}")]
    TooShort { need: usize, got: usize },
    #[error("key mismatch: expected {expected}, got {got}")]
    KeyMismatch { expected: u32, got: u32 },
    #[error("header fixed_len mismatch: expected {expected}, got {got}")]
    FixedLenMismatch { expected: usize, got: usize },
    #[error("extension block v{version} missing or malformed")]
    ExtBlock { version: u16 },
    #[error("slot out of bounds (off={off} len={len}, payload={end})")]
    SlotOutOfBounds { off: usize, len: usize, end: usize },
    #[error("invalid utf-8")]
    InvalidUtf8,
    #[error("unknown {enum_name} value {value}")]
    UnknownEnumValue { enum_name: &'static str, value: i32 },
    #[error("at `{field}`{}: {source}", .index.map(|i| format!("[{i}]")).unwrap_or_default())]
    Nested { field: &'static str, index: Option<usize>, source: Box<ValidationError> },
}

impl ValidationError {
    /// Wrap `self` as having been found inside `field` (element `index` for arrays and maps).
    pub fn nested(self, field: &'static str, index: Option<usize>) -> Self {
        ValidationError::Nested { field, index, source: Box::new(self) }
    }
}
