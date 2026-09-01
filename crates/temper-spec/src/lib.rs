//! temper-spec: Specification parsers for the Temper framework.
//!
//! Supports two specification formats:
//! - **I/O Automaton TOML** (primary): Lynch-Tuttle precondition/effect style, agent-friendly
//! - **TLA+** (legacy): temporal logic for deep correctness reasoning
//! - **CSDL** (data model): OData v4 Common Schema Definition Language
//!
//! Both I/O Automaton and TLA+ compile to the same [`StateMachine`] intermediate
//! representation, which feeds the verification cascade and runtime.

pub mod automaton;
pub mod bundle;
pub mod canonical;
pub mod cross_invariant;
pub mod csdl;
#[path = "model/mod.rs"]
pub mod legacy_model;
pub mod naming;

/// TLA+ specification extractor (legacy — prefer [`automaton`] for new specs).
pub mod tlaplus;

// Re-export primary public API at crate root.
pub use automaton::{
    Automaton, FieldInvariant, FieldPredicate, LintFinding, LintSeverity, lint_automaton,
    parse_automaton, parse_bool_initial, parse_counter_initial_usize, parse_list_initial,
    parse_var_initial_json, to_state_machine,
};
pub use bundle::{
    BundleError, BundleErrorCode, CanonicalIoaSpec, IoaSourceInput, MigrationArtifactInput,
    PolicyArtifactInput, ScopedBundleBudgets, ScopedSpecBundle, ScopedSpecBundleInput,
    WasmArtifactInput, scoped_module_data_closure_digest,
    scoped_module_data_closure_digest_with_version,
};
pub use canonical::{
    CanonicalActionContract, CanonicalActionParameter, CanonicalEntityModel, CanonicalSpecModel,
};
pub use cross_invariant::{
    CrossInvariant, CrossInvariantLintFinding, CrossInvariantLintSeverity, CrossInvariantOperator,
    CrossInvariantParseError, CrossInvariantSpec, DeletePolicy, InvariantKind, RelatedFieldAssert,
    RelationOverride, lint_cross_invariants, parse_cross_invariants, parse_related_field_assert,
    parse_related_status_in_assert,
};
pub use csdl::{CsdlDocument, CsdlParseError, parse_csdl};
pub use legacy_model::{
    LegacySpecModel, LegacySpecSource, build_legacy_spec_model, build_legacy_spec_model_mixed,
};
pub use naming::{to_pascal_case, to_snake_case};
pub use tlaplus::{Invariant, StateMachine, Transition, extract_state_machine};
