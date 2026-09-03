//! Versioned module-data failure contracts.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use temper_failure::{
    BoundedDiagnostic, BoundedFailureDetails, DetailKey, FailureCategory, FailureContractError,
    FailureDetailValue, FailureOutcome, FailureRetryability, GuestFailureDeclarationV1,
    ProvenanceToken, StableFailureCode,
};

use super::super::{DataObject, Retryability};

/// Maximum application-data-owned scalar detail entries.
pub const MAX_MODULE_DATA_DETAIL_ENTRIES: usize = 13;
/// Maximum serialized bytes of application-data-owned details.
pub const MAX_MODULE_DATA_DETAILS_SERIALIZED_BYTES: usize = 1_536;

const DECISION_ID_KEY: &str = "decision_id";
const DIAGNOSTIC_OMITTED_KEY: &str = "diagnostic_omitted";
const DETAILS_OMITTED_KEY: &str = "details_omitted";

/// Structured, validated error returned by application-data ABI v2.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleDataError {
    kind: ModuleDataErrorKind,
    code: StableFailureCode,
    diagnostic: Option<BoundedDiagnostic>,
    diagnostic_omitted: bool,
    retryability: FailureRetryability,
    outcome: FailureOutcome,
    decision_id: Option<ProvenanceToken>,
    details: BoundedFailureDetails,
    details_omitted: bool,
}

impl ModuleDataError {
    /// Construct a validated error, omitting an oversized optional diagnostic with evidence.
    pub fn new(
        kind: ModuleDataErrorKind,
        code: impl Into<String>,
        diagnostic: impl Into<String>,
        retryability: FailureRetryability,
        outcome: FailureOutcome,
    ) -> Result<Self, FailureContractError> {
        let code = StableFailureCode::new(code)?;
        validate_semantics(kind, &code, retryability, outcome)?;
        let diagnostic = diagnostic.into();
        let (diagnostic, diagnostic_omitted) = match BoundedDiagnostic::new(diagnostic) {
            Ok(value) => (Some(value), false),
            Err(FailureContractError::FieldTooLong { .. }) => (None, true),
            Err(error) => return Err(error),
        };
        Ok(Self {
            kind,
            code,
            diagnostic,
            diagnostic_omitted,
            retryability,
            outcome,
            decision_id: None,
            details: BoundedFailureDetails::default(),
            details_omitted: false,
        })
    }

    /// Return the closed error kind.
    pub const fn kind(&self) -> ModuleDataErrorKind {
        self.kind
    }

    /// Return the stable failure code.
    pub fn code(&self) -> &StableFailureCode {
        &self.code
    }

    /// Return the bounded diagnostic, when retained.
    pub fn diagnostic(&self) -> Option<&BoundedDiagnostic> {
        self.diagnostic.as_ref()
    }

    /// Report whether a diagnostic was omitted because it exceeded its budget.
    pub const fn diagnostic_omitted(&self) -> bool {
        self.diagnostic_omitted
    }

    /// Return retry guidance.
    pub const fn retryability(&self) -> FailureRetryability {
        self.retryability
    }

    /// Return the host-owned commit outcome.
    pub const fn outcome(&self) -> FailureOutcome {
        self.outcome
    }

    /// Return the bounded governance decision identity, when present.
    pub fn decision_id(&self) -> Option<&ProvenanceToken> {
        self.decision_id.as_ref()
    }

    /// Return application-data-owned bounded details.
    pub fn details(&self) -> &BoundedFailureDetails {
        &self.details
    }

    /// Report whether source details were omitted because they exceeded their budget.
    pub const fn details_omitted(&self) -> bool {
        self.details_omitted
    }

    /// Attach one validated governance decision identity.
    pub fn with_decision_id(
        mut self,
        decision_id: impl Into<String>,
    ) -> Result<Self, FailureContractError> {
        self.decision_id = Some(ProvenanceToken::new(decision_id)?);
        Ok(self)
    }

    /// Reclassify the host-owned outcome at a more authoritative commit boundary.
    pub fn with_outcome(mut self, outcome: FailureOutcome) -> Result<Self, FailureContractError> {
        self.outcome = outcome;
        if outcome == FailureOutcome::Unknown {
            self.retryability = FailureRetryability::Reconcile;
        } else if self.retryability == FailureRetryability::Reconcile {
            self.retryability = FailureRetryability::Never;
        }
        validate_error(&self)?;
        Ok(self)
    }

    /// Insert one application-data-owned scalar detail.
    pub fn try_insert_detail(
        &mut self,
        key: DetailKey,
        value: FailureDetailValue,
    ) -> Result<(), FailureContractError> {
        if is_reserved_key(key.as_str()) {
            return Err(FailureContractError::DetailsEncoding(format!(
                "module-data detail key {} is reserved",
                key.as_str()
            )));
        }
        let mut candidate = self.details.values().clone();
        candidate.insert(key.clone(), value.clone());
        validate_source_details(&candidate)?;
        self.details.try_insert(key, value)
    }

    /// Try to insert a detail, recording bounded omission instead of failing.
    pub fn insert_detail_or_omit(&mut self, key: DetailKey, value: FailureDetailValue) {
        if self.try_insert_detail(key, value).is_err() {
            self.details_omitted = true;
        }
    }

    /// Record that source metadata could not be represented in the scalar detail contract.
    pub fn mark_details_omitted(&mut self) {
        self.details_omitted = true;
    }
}

impl Serialize for ModuleDataError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        validate_error(self).map_err(serde::ser::Error::custom)?;
        #[derive(Serialize)]
        struct Wire<'a> {
            kind: ModuleDataErrorKind,
            code: &'a StableFailureCode,
            #[serde(skip_serializing_if = "Option::is_none")]
            diagnostic: &'a Option<BoundedDiagnostic>,
            diagnostic_omitted: bool,
            retryability: FailureRetryability,
            outcome: FailureOutcome,
            #[serde(skip_serializing_if = "Option::is_none")]
            decision_id: &'a Option<ProvenanceToken>,
            details: &'a BoundedFailureDetails,
            details_omitted: bool,
        }
        Wire {
            kind: self.kind,
            code: &self.code,
            diagnostic: &self.diagnostic,
            diagnostic_omitted: self.diagnostic_omitted,
            retryability: self.retryability,
            outcome: self.outcome,
            decision_id: &self.decision_id,
            details: &self.details,
            details_omitted: self.details_omitted,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ModuleDataError {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            kind: ModuleDataErrorKind,
            code: StableFailureCode,
            #[serde(default)]
            diagnostic: Option<BoundedDiagnostic>,
            diagnostic_omitted: bool,
            retryability: FailureRetryability,
            outcome: FailureOutcome,
            #[serde(default)]
            decision_id: Option<ProvenanceToken>,
            #[serde(default)]
            details: BoundedFailureDetails,
            details_omitted: bool,
        }
        let wire = Wire::deserialize(deserializer)?;
        let error = Self {
            kind: wire.kind,
            code: wire.code,
            diagnostic: wire.diagnostic,
            diagnostic_omitted: wire.diagnostic_omitted,
            retryability: wire.retryability,
            outcome: wire.outcome,
            decision_id: wire.decision_id,
            details: wire.details,
            details_omitted: wire.details_omitted,
        };
        validate_error(&error).map_err(de::Error::custom)?;
        Ok(error)
    }
}

impl core::fmt::Display for ModuleDataError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.code.as_str())?;
        if let Some(diagnostic) = &self.diagnostic {
            write!(f, ": {}", diagnostic.as_str())?;
        }
        Ok(())
    }
}

impl std::error::Error for ModuleDataError {}

/// Exact historical application-data ABI-v1 error wire view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModuleDataErrorV1 {
    /// Closed low-cardinality category.
    pub kind: ModuleDataErrorKind,
    /// Stable machine-readable code.
    pub code: String,
    /// Historical safe explanation field.
    pub message: String,
    /// Historical retry guidance.
    pub retryability: Retryability,
    /// Optional governance decision identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision_id: Option<String>,
    /// Optional historical metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Box<DataObject>>,
}

impl From<&ModuleDataError> for ModuleDataErrorV1 {
    fn from(error: &ModuleDataError) -> Self {
        let details = if error.details.values().is_empty() {
            None
        } else {
            Some(Box::new(
                error
                    .details
                    .values()
                    .iter()
                    .map(|(key, value)| (key.as_str().to_string(), value.to_json_scalar()))
                    .collect(),
            ))
        };
        Self {
            kind: error.kind,
            code: error.code.as_str().to_string(),
            message: error
                .diagnostic
                .as_ref()
                .map_or_else(String::new, |value| value.as_str().to_string()),
            retryability: legacy_retryability(error.retryability),
            decision_id: error
                .decision_id
                .as_ref()
                .map(|value| value.as_str().to_string()),
            details,
        }
    }
}

impl TryFrom<ModuleDataErrorV1> for ModuleDataError {
    type Error = FailureContractError;

    fn try_from(error: ModuleDataErrorV1) -> Result<Self, Self::Error> {
        let mut promoted = Self::new(
            error.kind,
            error.code,
            error.message,
            FailureRetryability::Reconcile,
            FailureOutcome::Unknown,
        )?;
        if let Some(decision_id) = error.decision_id {
            promoted = promoted.with_decision_id(decision_id)?;
        }
        if error.details.is_some() {
            promoted.details_omitted = true;
        }
        Ok(promoted)
    }
}

impl From<ModuleDataError> for GuestFailureDeclarationV1 {
    fn from(error: ModuleDataError) -> Self {
        let category = if error.outcome == FailureOutcome::Unknown {
            FailureCategory::Ambiguous
        } else {
            category_for(error.kind)
        };
        let retryability = if error.outcome == FailureOutcome::Unknown {
            FailureRetryability::Reconcile
        } else {
            error.retryability
        };
        let mut details = error.details;
        let reserved = [
            (
                DECISION_ID_KEY,
                error.decision_id.map(|value| {
                    FailureDetailValue::String(
                        temper_failure::BoundedDetailString::new(value.as_str())
                            .expect("provenance token fits the detail-string budget"),
                    )
                }),
            ),
            (
                DIAGNOSTIC_OMITTED_KEY,
                Some(FailureDetailValue::Bool(error.diagnostic_omitted)),
            ),
            (
                DETAILS_OMITTED_KEY,
                Some(FailureDetailValue::Bool(error.details_omitted)),
            ),
        ];
        for (key, value) in reserved {
            if let Some(value) = value {
                details
                    .try_insert(
                        DetailKey::new(key).expect("reserved detail key must be valid"),
                        value,
                    )
                    .expect("module-data source budgets reserve conversion capacity");
            }
        }
        GuestFailureDeclarationV1 {
            version: temper_failure::FAILURE_ENVELOPE_VERSION_V1,
            category,
            code: error.code,
            retryability,
            outcome: error.outcome,
            diagnostic: error.diagnostic,
            details,
        }
    }
}

/// Closed module-data error taxonomy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModuleDataErrorKind {
    /// Malformed or unsupported caller input.
    InvalidRequest,
    /// Input or binding differs from the verified schema.
    SchemaMismatch,
    /// Requested resource does not exist.
    NotFound,
    /// Resource already exists.
    AlreadyExists,
    /// Capability or Cedar authorization denied the operation.
    AuthorizationDenied,
    /// An IOA guard rejected the action.
    GuardRejected,
    /// A declared relation rejected the mutation.
    RelationViolation,
    /// Verification state does not permit the operation.
    VerificationFailed,
    /// Exact-sequence or other concurrency conflict.
    Conflict,
    /// Requested committed state cannot be observed within the bounded path.
    ConsistencyUnavailable,
    /// A declared operation or byte budget was exhausted.
    BudgetExceeded,
    /// A transient dependency is unavailable.
    TransientUnavailable,
    /// Safe internal failure without sensitive details.
    Internal,
}

/// Return the stable guest-facing category for a closed module-data error kind.
pub(crate) const fn category_for(kind: ModuleDataErrorKind) -> FailureCategory {
    match kind {
        ModuleDataErrorKind::InvalidRequest
        | ModuleDataErrorKind::SchemaMismatch
        | ModuleDataErrorKind::NotFound
        | ModuleDataErrorKind::AlreadyExists
        | ModuleDataErrorKind::GuardRejected
        | ModuleDataErrorKind::RelationViolation
        | ModuleDataErrorKind::VerificationFailed
        | ModuleDataErrorKind::Conflict => FailureCategory::Integrity,
        ModuleDataErrorKind::AuthorizationDenied => FailureCategory::Authorization,
        ModuleDataErrorKind::ConsistencyUnavailable | ModuleDataErrorKind::TransientUnavailable => {
            FailureCategory::Transient
        }
        ModuleDataErrorKind::BudgetExceeded => FailureCategory::Budget,
        ModuleDataErrorKind::Internal => FailureCategory::Permanent,
    }
}

fn validate_semantics(
    kind: ModuleDataErrorKind,
    code: &StableFailureCode,
    retryability: FailureRetryability,
    outcome: FailureOutcome,
) -> Result<(), FailureContractError> {
    GuestFailureDeclarationV1::new(category_for(kind), code.clone(), retryability, outcome)
        .map(|_| ())
}

fn validate_error(error: &ModuleDataError) -> Result<(), FailureContractError> {
    if error.diagnostic_omitted && error.diagnostic.is_some() {
        return Err(FailureContractError::ContradictoryOmission {
            field: "diagnostic",
        });
    }
    validate_semantics(error.kind, &error.code, error.retryability, error.outcome)?;
    validate_source_details(error.details.values())
}

fn validate_source_details(
    values: &BTreeMap<DetailKey, FailureDetailValue>,
) -> Result<(), FailureContractError> {
    if values.len() > MAX_MODULE_DATA_DETAIL_ENTRIES {
        return Err(FailureContractError::TooManyDetails {
            max: MAX_MODULE_DATA_DETAIL_ENTRIES,
            actual: values.len(),
        });
    }
    if let Some(key) = values.keys().find(|key| is_reserved_key(key.as_str())) {
        return Err(FailureContractError::DetailsEncoding(format!(
            "module-data detail key {} is reserved",
            key.as_str()
        )));
    }
    let bytes = serde_json::to_vec(values)
        .map_err(|error| FailureContractError::DetailsEncoding(error.to_string()))?;
    if bytes.len() > MAX_MODULE_DATA_DETAILS_SERIALIZED_BYTES {
        return Err(FailureContractError::DetailsTooLarge {
            max: MAX_MODULE_DATA_DETAILS_SERIALIZED_BYTES,
            actual: bytes.len(),
        });
    }
    Ok(())
}

fn is_reserved_key(key: &str) -> bool {
    matches!(
        key,
        DECISION_ID_KEY | DIAGNOSTIC_OMITTED_KEY | DETAILS_OMITTED_KEY
    )
}

const fn legacy_retryability(retryability: FailureRetryability) -> Retryability {
    match retryability {
        FailureRetryability::Never
        | FailureRetryability::AfterAuthorization
        | FailureRetryability::Reconcile => Retryability::Never,
        FailureRetryability::AfterRefresh => Retryability::AfterRefresh,
        FailureRetryability::WithBackoff => Retryability::WithBackoff,
    }
}
