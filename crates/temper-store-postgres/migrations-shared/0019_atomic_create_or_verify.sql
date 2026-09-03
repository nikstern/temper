-- Converge the published fork-only 0014 bootstrap coordinator schema onto
-- legacy and fixed-upstream lineages without rewriting migration history.
CREATE TABLE IF NOT EXISTS schema_bootstrap_operations (
    tenant             TEXT  NOT NULL,
    caller_authority   TEXT  NOT NULL,
    idempotency_key    TEXT  NOT NULL,
    operation_json     JSONB NOT NULL,
    PRIMARY KEY (tenant, caller_authority, idempotency_key)
);

CREATE TABLE IF NOT EXISTS schema_bootstrap_targets (
    tenant                  TEXT NOT NULL,
    scope_kind              TEXT NOT NULL,
    scope_id                TEXT NOT NULL,
    bundle_digest           TEXT NOT NULL,
    entity_type             TEXT NOT NULL,
    entity_id               TEXT NOT NULL,
    owner_caller_authority  TEXT NOT NULL,
    owner_idempotency_key   TEXT NOT NULL,
    PRIMARY KEY (tenant, scope_kind, scope_id, bundle_digest, entity_type, entity_id)
);

ALTER TABLE schema_bootstrap_operations ENABLE ROW LEVEL SECURITY;
ALTER TABLE schema_bootstrap_targets ENABLE ROW LEVEL SECURITY;

DROP POLICY IF EXISTS schema_bootstrap_operations_tenant_isolation ON schema_bootstrap_operations;
CREATE POLICY schema_bootstrap_operations_tenant_isolation ON schema_bootstrap_operations
    USING (tenant = current_setting('app.tenant_id', true))
    WITH CHECK (tenant = current_setting('app.tenant_id', true));

DROP POLICY IF EXISTS schema_bootstrap_targets_tenant_isolation ON schema_bootstrap_targets;
CREATE POLICY schema_bootstrap_targets_tenant_isolation ON schema_bootstrap_targets
    USING (tenant = current_setting('app.tenant_id', true))
    WITH CHECK (tenant = current_setting('app.tenant_id', true));

-- ADR-0196: immutable sequence-1 creation contracts and module-scoped retries.
CREATE TABLE IF NOT EXISTS entity_creation_contracts (
    tenant        TEXT  NOT NULL,
    entity_type   TEXT  NOT NULL,
    entity_id     TEXT  NOT NULL,
    contract_json JSONB NOT NULL,
    contract_digest TEXT NOT NULL,
    notification_pending BOOLEAN NOT NULL DEFAULT FALSE,
    contract_revision BIGINT NOT NULL,
    schema_identity TEXT NOT NULL,
    declared_key_signature TEXT NOT NULL,
    source_write_version BIGINT NOT NULL,
    PRIMARY KEY (tenant, entity_type, entity_id)
);

CREATE TABLE IF NOT EXISTS entity_create_or_verify_idempotency (
    tenant          TEXT NOT NULL,
    module_name     TEXT NOT NULL,
    entity_type     TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    entity_id       TEXT NOT NULL,
    requested_entity_id TEXT NOT NULL,
    requested_contract_json JSONB NOT NULL,
    contract_digest TEXT NOT NULL,
    PRIMARY KEY (tenant, module_name, entity_type, idempotency_key)
);

ALTER TABLE entity_create_or_verify_idempotency
    ADD COLUMN IF NOT EXISTS notification_pending BOOLEAN NOT NULL DEFAULT FALSE;

CREATE INDEX IF NOT EXISTS idx_eki_entity
    ON entity_key_index(tenant, entity_type, entity_id);

CREATE TABLE IF NOT EXISTS entity_creation_coverage (
    tenant                 TEXT NOT NULL,
    entity_type            TEXT NOT NULL,
    schema_identity        TEXT NOT NULL,
    contract_revision      BIGINT NOT NULL,
    declared_key_signature TEXT NOT NULL,
    cursor                 TEXT NOT NULL,
    source_write_version   BIGINT NOT NULL,
    covered_write_version  BIGINT NOT NULL,
    completed_at           TIMESTAMPTZ,
    PRIMARY KEY (
        tenant, entity_type, schema_identity, contract_revision, declared_key_signature
    )
);
