# ADR-0195: Default-Aware Borrowed Module SDK Commands

- Status: Accepted
- Date: 2026-09-01
- Deciders: Temper core maintainers
- Related:
  - ADR-0157: Metadata-Generated Typed Module Data SDK
  - ADR-0179: Canonical Entity-Valued Action Results
  - ADR-0184: Grant-Scoped Module SDK Surface
  - ADR-0185: Canonical Schema Default Materialization
  - ADR-0186: Canonical Property Provenance
  - ADR-0193: IOA Action Parameter Requiredness
  - ADR-0194: IOA-Authoritative Canonical Entity Model
  - GitHub issue #92
  - `crates/temper-codegen/src/module_sdk/`
  - `crates/temper-server/src/application_data/`
  - `crates/temper-wasm-sdk/src/data/`

## Context

The metadata-generated module SDK currently uses one owned entity-shaped Rust
struct for create input, a second all-optional owned struct for patch input,
and owned action input structs. Create therefore requires callers to initialize
canonical response fields that are nullable, defaulted, lifecycle-owned, or
server-managed. Every string, identifier, digest, and typed reference is moved
into a command which the generated client then consumes, so callers clone
values they still need.

Canonical response provenance does not define write admission. In particular,
`EntityId` is host-owned when projecting a response but remains an explicit,
caller-supplied deterministic create input. Conversely, `StoredField` says
where a response value is stored but cannot say whether callers may create or
patch it. The host currently accepts every known, correctly typed property in
both operations, so a handcrafted ABI request can bypass any narrower generated
surface.

Generated request encoding also serializes into an owned JSON value, borrows
its object, clones that map, and silently replaces an unexpected non-object
with an empty object. Write and action acknowledgements expose a commit token,
a raw optional value, and an omission boolean. Callers repeatedly reconstruct
the difference between a present value, a deliberate response-budget omission,
and malformed absence. Returning an ordinary missing-result error is unsafe
because losing the commit token can make retrying an already committed action
appear valid.

ADR-0194 has now made IOA lifecycle states canonical and generated closed Rust
lifecycle types. This decision consumes that completed contract and does not
create another lifecycle representation.

## Decision

### Write admission is orthogonal to response provenance

Every newly generated manifest property carries an explicit operation policy.
The serialized field is optional only so historical artifact bytes remain
readable; current generation always emits `Some`:

```rust
pub struct ManifestPropertyWritePolicyV1 {
    pub create: ManifestCreateRoleV1,
    pub patch: ManifestPatchRoleV1,
}

pub enum ManifestCreateRoleV1 {
    Required,
    Optional,
    Forbidden,
}

pub enum ManifestPatchRoleV1 {
    Writable,
    Forbidden,
}

pub struct ManifestPropertyV1 {
    // Existing fields omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub write_policy: Option<ManifestPropertyWritePolicyV1>,
}
```

The policy is separate from `ManifestValueSourceV1`, nullability, and the
canonical default. Their product defines the complete contract:

- the entity key is create-required and patch-forbidden under the current
  deterministic-ID contract, even though its response source is `EntityId`;
- the lifecycle property is forbidden for create and patch and is projected
  from `LifecycleStatus`;
- a caller-owned stored field is create-required when it is non-nullable and
  has no canonical default, otherwise create-optional;
- a server-managed stored field is create-forbidden;
- patchability is explicit and independent of create requiredness.

The canonical structural CSDL declares caller ownership through these exact
entity annotations:

- `Temper.Vocab.Write.CreateProperties` is an `AnnotationValue::Collection` of
  exact, case-sensitive CSDL stored-property names;
- `Temper.Vocab.Write.PatchProperties` is an `AnnotationValue::Collection` of
  exact, case-sensitive CSDL stored-property names.

The annotations must either both be absent or both be present, including as an
explicit empty collection. Bundle linking rejects a one-sided declaration,
unknown or duplicate names, and key/lifecycle entries. When present, each set
is closed: an unlisted stored property is forbidden for that operation. A
listed create property is required when it is non-nullable and has no canonical
default, otherwise optional.

In annotation-free structural CSDL, current bundle linking derives the prior
stored-field contract: stored fields are creatable and patchable, requiredness
comes from nullability/defaults, the key remains create-required, and lifecycle
remains host-owned. Current generation still serializes the resulting explicit
policy. Absence of serialized policy is reserved for authenticated historical
artifact manifests, not for newly linked schemas.

This derived policy is included in manifest binding and used-symbol digests.
The host validates the manifest policy for both generated requests and
handcrafted ABI requests before authorization, prechecks, or actor dispatch.

**Why this approach**: provenance, requiredness, defaults, and admission answer
different questions. Keeping the axes separate represents the deterministic-ID
exception directly and prevents code generation and host validation from
inventing different rules.

### Generated commands borrow while canonical values remain owned

Generated entity responses and entity IDs remain owned, deserializable values.
Create, patch, and action inputs become lifetime-parameterized command types.
String-like fields use borrowed string values; typed entity references use
generated borrowed ID-reference wrappers that can be constructed from an owned
generated ID or from `&str`. Scalar copy types and closed enums remain values.

Generated client methods borrow command structs for the duration of the
synchronous host call. Serialization is the sole conversion into the owned host
ABI object. A caller can therefore borrow from an owned `String`, digest, or
entity ID and use that owned value after the call without cloning.

Create commands expose a deterministic constructor containing only
create-required fields. Create-optional fields default to omission and have
chainable typed setters. The serializer never materializes canonical defaults;
omission leaves the host as their authority.

Action command types follow the same rule using ADR-0193's canonical parameter
requiredness: the constructor contains non-nullable parameters without a
canonical default, while nullable or defaulted parameters are omitted until a
typed setter supplies them. Generated code never materializes action defaults;
the host remains their authority. This replaces action struct literals as part
of the same regeneration break as create and patch commands.

Patch commands default every field to unchanged. A non-nullable patchable field
uses `Option<T>` where `None` means unchanged. A nullable patchable field uses a
shared, transparently serialized three-state command value:

```rust
pub enum NullablePatch<T> {
    Unchanged,
    Null,
    Value(T),
}
```

`Unchanged` is skipped, `Null` serializes as JSON null, and `Value` serializes
as its inner value. Forbidden fields are absent from the command type.

**Why this approach**: borrowing is confined to synchronous commands and does
not infect owned responses, durable state, or the host ABI. Constructors make
requiredness visible at compile time while omission preserves canonical host
defaults.

### One fail-closed command encoder owns the ABI conversion

The guest SDK provides one command-object encoder used by all generated create,
patch, and action methods. It serializes the borrowed command and extracts
`serde_json::Value::Object` by move. Any other shape returns a stable
`SchemaMismatch` error with a generated-command-shape code. It never borrows and
clones the map and never substitutes an empty object.

**Why this approach**: generated structs should always encode as objects, so a
different shape is a generator or serialization contract failure rather than
an empty request.

### Result cardinality and response presence are separate contracts

New action manifests carry a closed result cardinality derived from the
canonical CSDL return contract. As with write policy, optional serialization is
only a historical-byte compatibility mechanism; current generation always
emits `Some`:

```rust
pub enum ManifestActionResultCardinalityV1 {
    Void,
    Required,
    Nullable,
}

pub struct ManifestActionV1 {
    // Existing fields omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_cardinality: Option<ManifestActionResultCardinalityV1>,
}
```

Generated required, nullable, and void action methods retain distinct Rust
types. A nullable result preserves JSON null separately from transport absence.
At runtime, write and action helpers return commit-bearing success or
commit-bearing absence outcomes. Absence distinguishes deliberate budget
omission from malformed or unexpected absence. No helper converts either case
into an error that discards the `CommitToken`.

Required write values and required action results have consuming helpers that
return a committed typed value when present. Nullable actions return committed
`Some`, committed `None`, deliberate omission, or unexpected absence. Void
actions succeed without fabricating a value, while an omission flag on a void
result is malformed.

**Why this approach**: result type cardinality comes from schema; response
presence comes from transport and response budgeting. A nested `Option` can
represent both mechanically, but named committed outcomes make unsafe retry
decisions difficult to express accidentally.

### Authoritative readback uses existing sequence-aware keyed reads

Generated entity clients expose `get_at_least` over the existing
`EntityGet.at_least_sequence` ABI. Write acknowledgements can recover an omitted
entity value by reading the committed entity ID at or above the commit sequence.
Entity-valued bound actions currently return the same bound entity under
ADR-0179, so their omitted result uses the same commit token and readback path.

The normal client continues recording observed commit sequences automatically.
Explicit helpers accept the token so recovery remains correct even across
client instances. Scalar and enum action results have no authoritative entity
source and remain explicit committed-but-omitted outcomes.

**Why this approach**: the ABI already carries the necessary read fence. A new
operation or polling protocol would duplicate the consistency contract, while
fabricating omitted scalar results would be false.

### Compatibility is artifact-scoped and regeneration is explicit

The transport remains application-data ABI v1. `ModuleSdkManifest` gains an
optional, skip-serialized `contract_version`; current generation emits version
2 together with explicit write policies and result cardinalities. Version 2
publication rejects any missing policy or cardinality. Those emitted fields are
included in deterministic binding and used-symbol digests.

Historical manifests have no contract version, write policies, or result
cardinalities. Their `None` fields are skipped during serialization so loading
and reserializing the raw manifest preserves its historical canonical JSON and
binding digest. Restart and activation first verify the immutable artifact and
raw manifest against the stored historical digest. Only after that
authentication succeeds does the host derive an in-memory effective legacy
policy from source, nullability, defaults, key, and lifecycle metadata. The raw
manifest is never normalized before digest verification or persisted back with
derived fields.

New artifact publication requires contract version 2, so a caller cannot submit
an unversioned manifest to obtain legacy admission. The legacy path is reachable
only from a pre-existing, host-recorded, digest-pinned artifact binding. Action
result cardinality is derived after authentication from historical
`result_type`: no type is void; a present type uses the historical decoder
contract and gains no new nullable helper until regeneration.

Regenerating a module produces contract version 2, the new public Rust command
API, and a new binding digest. There is no source-compatibility shim for old
generated create, patch, or action struct literals. Maintained downstream
modules are regenerated and updated in the same rollout rather than retaining
duplicate deprecated command types.

**Why this approach**: persisted valid artifacts remain runnable, while every
new artifact receives the closed policy. Source compatibility for generated
code would preserve the cloning and ownership defects this decision removes.

### Closed lifecycle types remain canonical

The lifecycle property continues using ADR-0194's IOA-derived closed Rust type
in entity responses, filters, comparisons, and helpers. It is absent from
create and patch commands because its write policy is forbidden. No new state
list or lifecycle-specific default is added to the manifest.

**Why this approach**: one canonical state set must feed verification, runtime,
metadata, and code generation.

## Rollout Plan

1. Extend canonical linking and manifest metadata with closed write policies
   and action-result cardinality, including deterministic and legacy artifact
   tests.
2. Enforce the emitted roles in host create and patch validation before actor
   dispatch, with handcrafted ABI bypass tests.
3. Add borrowed ID references, nullable patch values, the fail-closed encoder,
   and commit-preserving outcomes to the guest SDK.
4. Generate constructor-based borrowed commands and specialized result helpers,
   then update golden and compile fixtures.
5. Add `get_at_least` and authoritative recovery helpers using commit tokens.
6. Regenerate a representative real module and run its complete local flow,
   proving reduced initialization, clone, lifecycle-string, and result-unwrapping
   boilerplate.
7. Run deterministic generation, restart, host/runtime, full workspace, clippy,
   formatting, mandatory DST, and code-quality gates before merge.
8. Deploy the merged kernel and regenerated downstream module, execute the live
   flow, and verify transition, omission/readback, and error telemetry in
   Datadog.

## Readiness Gates

- Every newly generated manifest property has one create and one patch role.
- Generated commands and host admission reject the same forbidden fields.
- Defaulted and nullable values may be omitted without client materialization.
- Compile fixtures prove borrowed owned strings and IDs remain usable after
  create, patch, and action calls.
- Nullable patch fixtures prove unchanged, null, and value encode distinctly.
- Non-object command encoding fails with the stable structured error.
- Every missing-result path retains a commit token and an explicit reason.
- Entity omission recovery is sequence-aware; scalar omission remains
  unrecoverable and explicit.
- Lifecycle fields retain ADR-0194's closed generated types.
- Generated source, manifests, used-symbol sets, and digests are deterministic.
- Handcrafted ABI requests cannot write forbidden lifecycle or server-managed
  fields.

## Consequences

### Positive

- Generated module code stops duplicating host defaults and caller clones.
- The generated type surface and host enforce one write-ownership contract.
- Callers cannot accidentally discard commit evidence while handling omitted
  results.
- Patch nullability is explicit and type checked.
- Lifecycle comparisons stay closed and IOA-derived.

### Negative

- Regenerated module source requires mechanical migration to constructors,
  setters, and borrowed commands.
- Generated code has more lifetime parameters and borrowed reference wrappers.
- Structural CSDL gains write-ownership annotations that app generation must
  maintain.
- Manifest digests change whenever write policy or result cardinality changes.

### Risks

- A role derivation mismatch could reject valid writes or admit forbidden ones.
  Canonical derivation is shared into the manifest and host tests replay raw ABI
  requests against it.
- Borrowed type generation can produce confusing lifetimes or identifier
  collisions. Compile fixtures cover owned and borrowed strings, IDs, enums,
  nullable/defaulted fields, and multiple reference types.
- Nullable result absence can collapse into JSON null. The decoder tests all
  cardinality and omission combinations before typed conversion.
- Legacy artifact behavior could become an admission downgrade if caller-
  supplied manifests were trusted. Only host-built digest-pinned legacy
  bindings receive legacy derivation; new generation always emits roles.

### DST Compliance

- Canonical role derivation and validation iterate existing deterministic
  manifest vectors and ordered sets; no unordered collection is introduced in
  simulation-visible code.
- Host checks are pure schema/value validation with no clocks, randomness,
  ambient I/O, tasks, or threads.
- Sequence-aware recovery uses the existing explicit commit sequence and does
  not poll wall time.
- No `// determinism-ok` suppression is expected.

## Non-Goals

- Changing deterministic entity creation to generate IDs implicitly.
- Moving application-specific builders or lifecycle semantics into the kernel.
- Duplicating canonical defaults in generated clients.
- Recovering scalar action results without an authoritative source.
- Supporting action results that materialize a different entity than the bound
  entity; ADR-0179 continues to reject them.
- Weakening grants, schema pins, immutable bindings, Cedar authorization, or
  response budgets.

## Alternatives Considered

1. **Infer admission from `ManifestValueSourceV1`** — Rejected because entity ID
   creation and server-managed stored fields prove response authority is not
   write admission.
2. **Infer server ownership from IOA effects** — Rejected because fields may be
   both initialized by callers and updated by actions; effect analysis does not
   express authority.
3. **Use owned `Cow` commands only** — Rejected as the sole surface because
   moving an owned value into `Cow::Owned` still prevents its later use. Explicit
   borrowed construction guarantees the no-clone path.
4. **Encode nullable patching as plain `Option<T>`** — Rejected because it cannot
   distinguish unchanged from set-null.
5. **Return ordinary errors for missing results** — Rejected because an error
   that loses the commit token encourages unsafe retries.
6. **Recover omitted scalar results from an entity read** — Rejected because no
   canonical mapping from an arbitrary scalar result to entity state exists.
7. **Bump the transport ABI** — Rejected because the existing operation and
   response envelopes already carry object inputs, omission flags, commit
   tokens, and sequence-fenced reads; the missing contracts are manifest and
   generated API concerns.

## Rollback Policy

Revert canonical metadata, host enforcement, guest helpers, and generated code
together. Existing immutable pre-decision artifacts remain readable throughout.
Do not retain generated role restrictions without host enforcement, or host
enforcement without the exact role metadata included in the binding digest.
