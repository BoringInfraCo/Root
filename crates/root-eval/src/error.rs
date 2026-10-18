use thiserror::Error;

/// Machine-matchable classification for every [`TaskError`] variant.
///
/// The kind deliberately collapses payload details (field names, found values)
/// so callers and tests can assert the *rule* that refused a record without
/// matching on free-form messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorKind {
    /// The document is not valid TOML, or canonical re-serialization failed.
    Syntax,
    /// The input bytes were not valid UTF-8.
    NonUtf8,
    /// `schema_version` is outside the supported range.
    SchemaVersion,
    /// A table contained a field that is not part of the exact key set.
    UnknownField,
    /// A required field was absent.
    MissingField,
    /// A value had the wrong TOML type.
    TypeInvalid,
    /// The schema forbids floating-point values entirely.
    FloatRejected,
    /// A draft's `sha256:PENDING` sentinel was found in a digest field.
    SentinelRefusal,
    /// A digest is not `sha256:<64 lowercase hex>`.
    DigestInvalid,
    /// A string contains control characters where they are forbidden.
    ControlChars,
    /// A string that must be non-empty was empty.
    EmptyString,
    /// An enum-valued field holds a value outside its closed set.
    EnumInvalid,
    /// An identifier does not match the task-spec naming grammar.
    NamingRule,
    /// A path is absolute, escapes its root, or uses non-`/` separators.
    PathInvalid,
    /// A budget field violates the budget rules.
    BudgetRule,
    /// A prompt violates the prompt rules.
    PromptRule,
    /// A fixture violates the fixture rules.
    FixtureRule,
    /// An environment declaration violates the environment rules.
    EnvironmentRule,
    /// An acceptance assertion violates the acceptance rules.
    AcceptanceRule,
    /// A verifier declaration or reference violates the verifier rules.
    VerifierRule,
    /// A termination override violates the closed termination mapping.
    TerminationRule,
    /// A logical identifier or key occurs more than once.
    Duplicate,
    /// A numeric value is outside its permitted range.
    RangeInvalid,
    /// A content digest does not recompute from the parsed content.
    IntegrityMismatch,
    /// The canonical serialization is not a parse/re-serialize fixed point.
    CanonicalDrift,
}

/// Errors produced while parsing or validating a benchmark task record.
///
/// Parsing is split into two layers. TOML syntax and the exact key sets are
/// checked first, then semantic rules. The variants below preserve that
/// ordering and carry enough context for machine matching: every variant maps
/// to one [`ErrorKind`], and most carry the offending field path.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TaskError {
    #[error("TOML syntax error: {message}")]
    Syntax { message: String },

    #[error("task record bytes are not valid UTF-8")]
    NonUtf8,

    #[error(
        "task schema version {found} is not supported (this parser reads version {supported})"
    )]
    SchemaVersion { found: i64, supported: u32 },

    #[error("unknown field `{field}` in {location}")]
    UnknownField { field: String, location: String },

    #[error("missing required field `{field}`")]
    MissingField { field: String },

    #[error("invalid type for `{field}`: expected {expected}")]
    TypeInvalid { field: String, expected: String },

    #[error("float values are not permitted in task records (`{field}`)")]
    FloatRejected { field: String },

    #[error(
        "draft sentinel `sha256:PENDING` in `{field}`; draft records cannot be used for evaluation"
    )]
    SentinelRefusal { field: String },

    #[error("invalid digest in `{field}`: {reason}")]
    DigestInvalid { field: String, reason: String },

    #[error("control characters are not permitted in `{field}`")]
    ControlChars { field: String },

    #[error("`{field}` must not be empty")]
    EmptyString { field: String },

    #[error("invalid value `{value}` for `{field}`; allowed: {allowed}")]
    EnumInvalid {
        field: String,
        value: String,
        allowed: &'static str,
    },

    #[error("invalid identifier in `{field}`: {reason}")]
    NamingRule { field: String, reason: String },

    #[error("invalid path in `{field}`: {reason}")]
    PathInvalid { field: String, reason: String },

    #[error("invalid budget `{field}`: {reason}")]
    BudgetRule { field: String, reason: String },

    #[error("invalid prompt `{field}`: {reason}")]
    PromptRule { field: String, reason: String },

    #[error("invalid fixture `{field}`: {reason}")]
    FixtureRule { field: String, reason: String },

    #[error("invalid environment `{field}`: {reason}")]
    EnvironmentRule { field: String, reason: String },

    #[error("invalid acceptance `{field}`: {reason}")]
    AcceptanceRule { field: String, reason: String },

    #[error("invalid verifier `{field}`: {reason}")]
    VerifierRule { field: String, reason: String },

    #[error("invalid termination mapping `{field}`: {reason}")]
    TerminationRule { field: String, reason: String },

    #[error("duplicate value `{value}` for `{field}`")]
    Duplicate { field: String, value: String },

    #[error("value out of range for `{field}`: {reason}")]
    RangeInvalid { field: String, reason: String },

    #[error("integrity mismatch for `{field}`: expected {expected}, found {found}")]
    IntegrityMismatch {
        field: String,
        expected: String,
        found: String,
    },

    #[error("canonical serialization is not a fixed point: {detail}")]
    CanonicalDrift { detail: String },
}

impl TaskError {
    /// The machine-matchable rule classification.
    pub fn kind(&self) -> ErrorKind {
        match self {
            Self::Syntax { .. } => ErrorKind::Syntax,
            Self::NonUtf8 => ErrorKind::NonUtf8,
            Self::SchemaVersion { .. } => ErrorKind::SchemaVersion,
            Self::UnknownField { .. } => ErrorKind::UnknownField,
            Self::MissingField { .. } => ErrorKind::MissingField,
            Self::TypeInvalid { .. } => ErrorKind::TypeInvalid,
            Self::FloatRejected { .. } => ErrorKind::FloatRejected,
            Self::SentinelRefusal { .. } => ErrorKind::SentinelRefusal,
            Self::DigestInvalid { .. } => ErrorKind::DigestInvalid,
            Self::ControlChars { .. } => ErrorKind::ControlChars,
            Self::EmptyString { .. } => ErrorKind::EmptyString,
            Self::EnumInvalid { .. } => ErrorKind::EnumInvalid,
            Self::NamingRule { .. } => ErrorKind::NamingRule,
            Self::PathInvalid { .. } => ErrorKind::PathInvalid,
            Self::BudgetRule { .. } => ErrorKind::BudgetRule,
            Self::PromptRule { .. } => ErrorKind::PromptRule,
            Self::FixtureRule { .. } => ErrorKind::FixtureRule,
            Self::EnvironmentRule { .. } => ErrorKind::EnvironmentRule,
            Self::AcceptanceRule { .. } => ErrorKind::AcceptanceRule,
            Self::VerifierRule { .. } => ErrorKind::VerifierRule,
            Self::TerminationRule { .. } => ErrorKind::TerminationRule,
            Self::Duplicate { .. } => ErrorKind::Duplicate,
            Self::RangeInvalid { .. } => ErrorKind::RangeInvalid,
            Self::IntegrityMismatch { .. } => ErrorKind::IntegrityMismatch,
            Self::CanonicalDrift { .. } => ErrorKind::CanonicalDrift,
        }
    }

    /// The offending field path, when the error is field-scoped.
    pub fn field(&self) -> Option<&str> {
        match self {
            Self::Syntax { .. } | Self::NonUtf8 | Self::CanonicalDrift { .. } => None,
            Self::SchemaVersion { .. } => Some("schema_version"),
            Self::UnknownField { field, .. }
            | Self::MissingField { field }
            | Self::TypeInvalid { field, .. }
            | Self::FloatRejected { field }
            | Self::SentinelRefusal { field }
            | Self::DigestInvalid { field, .. }
            | Self::ControlChars { field }
            | Self::EmptyString { field }
            | Self::EnumInvalid { field, .. }
            | Self::NamingRule { field, .. }
            | Self::PathInvalid { field, .. }
            | Self::BudgetRule { field, .. }
            | Self::PromptRule { field, .. }
            | Self::FixtureRule { field, .. }
            | Self::EnvironmentRule { field, .. }
            | Self::AcceptanceRule { field, .. }
            | Self::VerifierRule { field, .. }
            | Self::TerminationRule { field, .. }
            | Self::Duplicate { field, .. }
            | Self::RangeInvalid { field, .. }
            | Self::IntegrityMismatch { field, .. } => Some(field),
        }
    }

    /// True when the only obstacle to evaluation is an unsealed draft.
    pub fn is_draft_refusal(&self) -> bool {
        matches!(self, Self::SentinelRefusal { .. })
    }
}
