use thiserror::Error;

#[derive(Debug, Error)]
pub enum SchemaError {
    #[error("message key {0} is duplicated (keys must be globally unique and non-zero)")]
    DuplicateKey(u32),
    #[error("name `{0}` is defined more than once")]
    DuplicateName(String),
    #[error("unknown struct `{0}`: referenced before definition or not defined in this schema set")]
    UnknownType(String),
    #[error("inline struct cycle detected: {0:?} (recursion is only allowed through dynamic arrays)")]
    InlineCycle(Vec<String>),
    #[error("field `{0}`: invalid array/map element type (map cannot be an element in V1)")]
    InvalidElement(String),
    #[error("invalid map key type `{0}` (key must be a fixed-size scalar, enum, or string)")]
    InvalidMapKey(String),
    #[error("field `{0}`: since must be >= {1}")]
    InvalidSince(String, u16),
    #[error("message key must be non-zero")]
    ZeroKey,
    #[error("struct `{0}` has versioned (since >= 2) fields and cannot be referenced by other types in V1; evolve the containing message instead")]
    EvolvedStructInline(String),
}
