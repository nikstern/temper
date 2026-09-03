# ADR-0197: Module-Data Failure ABI v2

- Status: Proposed
- Date: 2026-09-03
- Deciders: Temper core maintainers
- Related:
  - ADR-0157: Metadata-Generated Typed Module Data SDK
  - ADR-0187: Versioned Application Failure Envelopes
  - ADR-0190: Typed WASM Guest Terminal Failures
  - ADR-0191: Host-Owned Typed Scoped Module Data
  - `crates/temper-failure`
  - `crates/temper-wasm-sdk/src/data/`
  - `crates/temper-server/src/application_data/`

## Context

Application-data ABI v1 carries a structured `ModuleDataError`, but its stable
code and diagnostic are unbounded strings, its details are arbitrary JSON, and
it does not carry the source-owned fact of whether an operation applied. The
application-facing adapter consequently accepts `FailureOutcome` separately
from the error. That makes contradictory classification representable and
forces a later layer, which may not know the commit boundary, to guess.

ADR-0190, implemented and accepted through PR #71, established the bounded
`GuestFailureDeclarationV1` terminal-failure contract. Issue #93 needs an
infallible `ModuleDataError` conversion into that declaration, which is sound
only if module-data errors already contain a validated, host-owned outcome.

Historical WASM artifacts embed ABI-v1 request and response handling. Their
exact successful and failed wire bytes must remain readable and executable
while new artifacts adopt a sound contract.

## Decision

### 1. Preserve ABI v1 Through An Explicit Legacy Wire View

The host continues to accept `DataRequestV1` and emit byte-compatible
`DataResponseV1` values. Its error wire view retains exactly the existing
fields and ordering: `kind`, `code`, `message`, `retryability`, optional
`decision_id`, and optional `details`. Golden tests pin request, successful
response, and error response bytes.

The in-memory canonical error is not serialized directly into v1. A separate
`ModuleDataErrorV1` wire type retains the historical fields without pretending
to be the canonical error. A dedicated v1 response adapter projects bounded
canonical fields into that legacy type. Outcomes and omission evidence do not
appear on the v1 wire.

The retryability projection is exact for `Never`, `AfterRefresh`, and
`WithBackoff`. Both `AfterAuthorization` and `Reconcile` project to `Never`:
an old guest has neither an authorization-wait primitive nor a reconciliation
contract, and ordinary retry would be unsafe. If current code explicitly
promotes a decoded legacy error into the canonical type, the missing outcome is
conservatively represented as `Unknown` and its retryability becomes
`Reconcile`; it is never promoted as `NotApplied`.

**Why this approach**: A single derived serializer with optional new fields can
accidentally change historical bytes as soon as a new field becomes populated.
An explicit view makes the compatibility boundary reviewable and testable.

### 2. Add ABI v2 With One Canonical Error

`DataRequestV2` carries ABI version 2 and the same closed operation vocabulary.
`DataResponseV2` carries the same successful results and a canonical
`ModuleDataError` containing:

- a closed `ModuleDataErrorKind`;
- a validated `StableFailureCode`;
- optional `BoundedDiagnostic` plus `diagnostic_omitted` evidence;
- closed `FailureRetryability`;
- host-owned `FailureOutcome`;
- optional bounded decision identity;
- `BoundedFailureDetails` plus `details_omitted` evidence.

Construction validates the canonical failure semantic rule: unknown outcome
if and only if retryability is reconciliation, and ambiguous classification is
derived only at the later declaration boundary. Over-budget optional data is
omitted with evidence; required stable codes and semantic combinations are
rejected. Serialization revalidates the contract and uses deterministic field
and detail ordering.

**Why this approach**: Reusing `temper-failure` primitives gives module data
the same budgets and scalar vocabulary as the terminal failure boundary, while
keeping provenance and causal operation identity kernel-owned.

### 3. Make The Commit Boundary An Error-Construction Input

Every server error construction site supplies its known outcome:

- request validation, authorization, reads, and rejected pre-commit writes use
  `NotApplied`;
- a known durable commit followed by response construction or projection
  failure uses `Applied`;
- a store or transport failure whose acknowledgement was lost uses `Unknown`
  with `Reconcile` retryability.

Persistence implementations surface `PreCommit`, `PostCommit`, or
`AcknowledgementUnknown` evidence when they know the phase. A typed state-layer
mutation boundary likewise preserves causal knowledge that an event was durable
before a later projection or response failure. Bare sequence comparison is not
causal evidence: an unchanged actor can be stale after acknowledgement loss, and
an advance can belong to another concurrent mutation. `Applied` requires causal
operation identity or structural evidence from the mutation boundary;
`NotApplied` requires a typed pre-commit result or causally fenced proof such as
durable absence for a newly allocated identity. If neither source can prove the
phase, the adapter must retain `Unknown`.

Before-commit PostgreSQL and Turso failures are `PreCommit`; a transaction
commit error or timed-out write is `AcknowledgementUnknown`; and a failure
after a non-transactional Turso event insert is `PostCommit`. Turso retries only
transient `PreCommit` failures. Actor startup carries its one-shot structural
phase back to the create coordinator, while new-File append and projection
failures retain their exact phase.

Batch item errors carry their own outcome. Batch-envelope admission errors are
not applied. Create-or-verify reservations and response compaction preserve a
known commit token and use applied outcome for failures after that token is
known. If any batch member remains `Unknown`, that uncertainty dominates the
aggregate response-budget failure even when another member has a known commit.
Error classification never parses diagnostics.

**Why this approach**: Only the layer adjacent to dispatch and persistence can
state whether a mutation crossed its commit boundary. Encoding that fact at
construction prevents later adapters from inventing it.

### 4. Define One Infallible Guest Declaration Conversion

`From<ModuleDataError> for GuestFailureDeclarationV1` derives the category
solely from the closed module-data kind. Unknown outcomes force
`ambiguous / reconcile / unknown`; all other outcomes preserve the mapped
category and retryability. The conversion preserves stable code, diagnostic,
bounded scalar details, omission evidence, and inserts a bounded decision ID
under the canonical `decision_id` detail key.

The details contract reserves the exact keys `decision_id`,
`diagnostic_omitted`, and `details_omitted`; source details containing one of
those keys are rejected. A module-data source map has at most 13 entries and at
most 1,536 serialized bytes. A decision ID is a `ProvenanceToken` (at most 64
ASCII token bytes), and both omission flags are always inserted as booleans.
The target contract permits 16 entries and 2,048 serialized bytes. The three
reserved tagged-scalar entries, including JSON punctuation, add at most 224
bytes, so every valid source map converts without eviction and remains at
least 288 bytes below the target byte budget. Construction enforces both the
entry reservation and source-byte budget before returning a canonical error.

The application-facing `FailureEnvelopeV1` adapter first performs this
conversion and adds only causal operation identity and module-data provenance.

**Why this approach**: One total mapping prevents SDK `?` conversion and server
failure routing from drifting. Reserving conversion-owned detail capacity is
what makes an infallible conversion honest rather than panic- or truncation-
based.

### 5. Negotiate By Request Version And Keep Artifact Authority

The host reads the bounded request ABI discriminator before decoding the
version-specific request. Version 1 is decoded and answered with v1; version 2
is decoded and answered with v2. Unknown versions fail closed in the same
version-neutral host transport path without attempting an operation.

Newly generated data clients and manifests use v2. Artifact-bound manifests
with ABI v1 remain valid and select the v1 client contract; artifacts cannot
claim one ABI and send another. Both versions share the existing request,
response, call, stream, and batch budgets.

**Why this approach**: The request selects the response decoder already linked
into the guest, while the verified artifact binding prevents the version field
from becoming an authority or capability selector.

## Rollout Plan

1. Land ABI-v1 goldens, v2 contracts, canonical conversion, and dual-version
   host handling without removing any v1 artifact path.
2. Audit all application-data operations and fault tests for explicit commit
   outcomes, including batch and create-or-verify.
3. Deploy the kernel and verify legacy data-bound modules before generating any
   typed-invocation artifacts that require v2.
4. Generate unified SDK bindings for issue #93 with zero or one v2 data grant.

## Readiness Gates

- Exact ABI-v1 request, success, and error bytes are unchanged.
- ABI-v2 bytes and deterministic round trips are pinned.
- Every error kind, retryability, and outcome maps exhaustively.
- Pre-commit, known-post-commit, and acknowledgement-unknown fault paths are
  covered for ordinary, batch, and create-or-verify operations.
- Legacy generated artifacts execute before and after server restart.
- Focused suites, workspace tests, strict clippy, formatting, integrity, DST,
  and code-quality reviews pass before merge.

## Consequences

### Positive

- Module-data failures carry enough trusted facts for sound typed propagation.
- Historical artifacts keep their exact protocol.
- Guest declarations and application envelopes share one classification path.
- Unknown commit state cannot be accidentally downgraded to an ordinary retry.

### Negative

- The host and SDK retain two response codecs until ABI v1 is retired.
- Every error-producing server path must identify its commit phase explicitly.
- Conversion-owned omission keys reduce the detail entries available to
  application-data-specific metadata.

### Risks

- A missed post-commit path could falsely claim `NotApplied`; exhaustive helper
  APIs and injected commit-boundary faults mitigate this.
- Compatibility could drift if v1 derives directly from the canonical type;
  golden bytes and the explicit legacy view prevent that.
- Dual-version decoding could admit ambiguous payloads; strict version-specific
  types with unknown-field rejection prevent cross-version fallback.

### DST Compliance

- Bounded details remain backed by `BTreeMap` and serialize deterministically.
- Outcome selection depends only on explicit execution facts, never timing,
  diagnostics, unordered iteration, or ambient process state.
- Fault tests use the deterministic actor/runtime harness and existing injected
  store outcomes.

## Non-Goals

- Typed invocation parameters, state projection, or success outcomes from #93.
- Changes to application domain failure routes.
- Removal of ABI v1 or migration of downstream ARC modules.
- Guest ownership of causal identity, provenance, or commit outcome.

## Alternatives Considered

1. **Add optional fields to the v1 error serializer** — Rejected because
   populated fields change historical bytes and old guests cannot interpret the
   semantics.
2. **Keep outcome as an adapter argument** — Rejected because it permits a
   caller without commit knowledge to contradict the source error.
3. **Infer outcome from kind or diagnostic text** — Rejected because the same
   dependency failure can occur before dispatch, after commit, or after an
   acknowledgement is lost.
4. **Return a fallible guest conversion** — Rejected because ordinary Rust `?`
   propagation would then require application-owned fallback classification.

## Rollback Policy

ABI-v1 remains executable throughout. Roll back any v2-generated application
artifacts before rolling the kernel back. The prerequisite deployment itself
introduces no production artifact that requires v2, so the server can otherwise
return to the preceding release without data migration.
