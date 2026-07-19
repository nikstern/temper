//! Credential-to-identity resolution.
//!
//! Hashes bearer tokens, looks up `AgentCredential` entities, verifies the
//! linked `AgentType` is active, and returns a `ResolvedIdentity` that the
//! security context uses as the authoritative agent identity.

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use temper_runtime::scheduler::sim_now;
use temper_runtime::tenant::TenantId;

use crate::identity::jwt;
use crate::state::ServerState;

/// Cache entry TTL in seconds.
const CACHE_TTL_SECS: i64 = 60;

/// Clock-skew tolerance (seconds) for JWT `exp`/`nbf` validation.
const JWT_LEEWAY_SECS: i64 = 60;

/// A platform-resolved agent identity.
///
/// All fields are derived from the credential registry — never from
/// self-declared headers or client-reported values.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedIdentity {
    /// Platform-assigned unique agent instance ID (UUIDv7) for the credential
    /// path; the token's `client_id` (acting agent) for the JWT path.
    pub agent_instance_id: String,
    /// The AgentType entity ID this credential is linked to. Empty for the JWT
    /// path, which takes its type name straight from a verified claim.
    pub agent_type_id: String,
    /// The AgentType's human-readable name (e.g., "claude-code").
    pub agent_type_name: String,
    /// Whether this identity was verified (registry or trusted-issuer JWT).
    pub verified: bool,
    /// JWT path only: the owning human (the agent's `acting_for` / token `sub`).
    #[serde(default)]
    pub acting_for: Option<String>,
    /// JWT path only: the sign-out-everywhere generation carried by the token,
    /// for the follow-up revocation check.
    #[serde(default)]
    pub auth_generation: Option<i64>,
    /// True when this identity came from a verified JWT rather than an
    /// `AgentCredential`. Selects the security-context constructor downstream.
    #[serde(default)]
    pub from_jwt: bool,
    /// JWT path only: true when the token represents a human (Customer) rather
    /// than an agent — i.e. it carries a `sub` but no `agent_type`.
    #[serde(default)]
    pub is_human: bool,
    /// JWT path only: the principal's role (owner/curator/contributor) from a
    /// verified token claim, for Cedar evaluation.
    #[serde(default)]
    pub role: Option<String>,
}

/// Cached resolution result with expiry.
struct CacheEntry {
    identity: ResolvedIdentity,
    expires_at: chrono::DateTime<chrono::Utc>,
}

/// Resolves bearer tokens to platform-assigned agent identities.
///
/// Uses an in-memory cache (`BTreeMap` for DST determinism) to avoid
/// entity lookups on every request. Cache entries are scoped by `(tenant,
/// key_hash)` so a credential verified in one tenant never leaks into
/// another. Entries expire after [`CACHE_TTL_SECS`].
pub struct IdentityResolver {
    cache: Arc<RwLock<BTreeMap<String, CacheEntry>>>,
}

impl Default for IdentityResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl IdentityResolver {
    /// Create a new identity resolver.
    pub fn new() -> Self {
        Self {
            cache: Arc::new(RwLock::new(BTreeMap::new())),
        }
    }

    /// Resolve a bearer token to a verified identity.
    ///
    /// JWT-shaped tokens (`header.payload.signature`) are verified against a
    /// registered [`TrustedIssuer`]; opaque tokens are looked up in the
    /// `AgentCredential` registry. Both paths share the in-memory cache.
    pub async fn resolve(
        &self,
        state: &ServerState,
        tenant: &TenantId,
        bearer_token: &str,
    ) -> Option<ResolvedIdentity> {
        let key_hash = hash_token(bearer_token);
        let cache_key = cache_key(tenant, &key_hash);

        // Check cache first.
        if let Some(cached) = self.get_cached(&cache_key) {
            return Some(cached);
        }

        if looks_like_jwt(bearer_token) {
            // JWT path: verify against the token's registered issuer. Cache no
            // longer than the token's own expiry so an expired token is never
            // served from cache.
            let (identity, exp_unix) = self.resolve_jwt(state, tenant, bearer_token).await?;
            let cap = sim_now() + chrono::Duration::seconds(CACHE_TTL_SECS);
            let token_exp = chrono::DateTime::from_timestamp(exp_unix, 0).unwrap_or(cap);
            self.put_cached_until(cache_key, identity.clone(), cap.min(token_exp));
            Some(identity)
        } else {
            let identity = self.resolve_credential(state, tenant, &key_hash).await?;
            self.put_cached(cache_key, identity.clone());
            Some(identity)
        }
    }

    /// Opaque-token path: look up the `AgentCredential` registry.
    ///
    /// key_hash is the entity ID (the `Issue` action uses it as such), giving
    /// an O(1) lookup. Verifies the credential and its linked `AgentType` are
    /// both `Active`.
    async fn resolve_credential(
        &self,
        state: &ServerState,
        tenant: &TenantId,
        key_hash: &str,
    ) -> Option<ResolvedIdentity> {
        let cred_response = state
            .get_tenant_entity_state(tenant, "AgentCredential", key_hash)
            .await
            .ok()?;

        if cred_response.state.status != "Active" {
            return None;
        }

        let fields = &cred_response.state.fields;
        let agent_type_id = fields.get("agent_type_id")?.as_str()?;
        let agent_instance_id = fields.get("agent_instance_id")?.as_str()?;

        if agent_type_id.is_empty() || agent_instance_id.is_empty() {
            return None;
        }

        let type_response = state
            .get_tenant_entity_state(tenant, "AgentType", agent_type_id)
            .await
            .ok()?;

        if type_response.state.status != "Active" {
            return None;
        }

        let agent_type_name = type_response
            .state
            .fields
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        Some(ResolvedIdentity {
            agent_instance_id: agent_instance_id.to_string(),
            agent_type_id: agent_type_id.to_string(),
            agent_type_name,
            verified: true,
            acting_for: None,
            auth_generation: None,
            from_jwt: false,
            is_human: false,
            role: None,
        })
    }

    /// JWT path: verify an ES256 token against its registered `TrustedIssuer`.
    ///
    /// Returns the resolved identity and the token's `exp` (used to cap cache
    /// TTL). The unverified `iss` claim only selects which issuer's keys to
    /// check against; the signature is the gate.
    async fn resolve_jwt(
        &self,
        state: &ServerState,
        tenant: &TenantId,
        token: &str,
    ) -> Option<(ResolvedIdentity, i64)> {
        // Read `iss` from the unverified payload to pick the issuer entity.
        let unverified = jwt::decode_claims_unverified(token).ok()?;
        let issuer_id = unverified.iss;

        let issuer_response = state
            .get_tenant_entity_state(tenant, "TrustedIssuer", &issuer_id)
            .await
            .ok()?;
        if issuer_response.state.status != "Active" {
            return None;
        }

        let fields = &issuer_response.state.fields;
        let jwks_json = fields.get("jwks_json")?.as_str()?;
        let audience = fields.get("audience")?.as_str()?;
        let jwks: jwt::Jwks = serde_json::from_str(jwks_json).ok()?;

        let now_unix = sim_now().timestamp();
        let claims = jwt::verify(
            token,
            &jwks,
            &issuer_id,
            audience,
            now_unix,
            JWT_LEEWAY_SECS,
        )
        .ok()?;

        // Sign-out-everywhere: reject a token whose generation is older than the
        // principal's current generation (RFC-0002, ARN-255 option A). The
        // generation is keyed on the human `sub`, so signing out a human also
        // invalidates the tokens of agents acting for them.
        if let Some(token_gen) = claims.auth_generation
            && let Some(sub) = claims.sub.as_deref()
            && token_gen < self.current_generation(state, tenant, sub).await
        {
            return None;
        }

        // Map verified claims → identity. A token with an `agent_type` is an
        // agent acting for the human `sub`; a token with only a `sub` is the
        // human themselves (a Customer principal).
        let identity = match claims.agent_type.as_deref().filter(|s| !s.is_empty()) {
            Some(agent_type) => {
                let client_id = claims.client_id.clone().unwrap_or_default();
                if client_id.is_empty() {
                    return None;
                }
                ResolvedIdentity {
                    agent_instance_id: client_id,
                    agent_type_id: String::new(),
                    agent_type_name: agent_type.to_string(),
                    verified: true,
                    acting_for: claims.sub.clone(),
                    auth_generation: claims.auth_generation,
                    from_jwt: true,
                    is_human: false,
                    role: claims.role.clone(),
                }
            }
            None => {
                let sub = claims.sub.clone().unwrap_or_default();
                if sub.is_empty() {
                    return None;
                }
                ResolvedIdentity {
                    agent_instance_id: sub,
                    agent_type_id: String::new(),
                    agent_type_name: String::new(),
                    verified: true,
                    acting_for: None,
                    auth_generation: claims.auth_generation,
                    from_jwt: true,
                    is_human: true,
                    role: claims.role.clone(),
                }
            }
        };
        Some((identity, claims.expiry()))
    }

    /// Read a principal's current sign-out-everywhere generation.
    ///
    /// Missing entity ⇒ generation 0 (never signed out everywhere). The counter
    /// lives in the kernel's `PrincipalGeneration` entity, so this check is
    /// generic across apps.
    async fn current_generation(&self, state: &ServerState, tenant: &TenantId, sub: &str) -> i64 {
        match state
            .get_tenant_entity_state(tenant, "PrincipalGeneration", sub)
            .await
        {
            Ok(resp) => resp
                .state
                .counters
                .get("generation")
                .map(|c| *c as i64)
                .unwrap_or(0),
            Err(_) => 0,
        }
    }

    /// Invalidate all cached entries (e.g., after credential rotation/revocation).
    pub fn invalidate_all(&self) {
        let mut cache = self.cache.write().unwrap(); // ci-ok: infallible lock
        cache.clear();
    }

    /// Invalidate a specific credential by its token.
    pub fn invalidate_token(&self, bearer_token: &str) {
        let key_hash = hash_token(bearer_token);
        self.invalidate_key_hash(&key_hash);
    }

    /// Invalidate a specific credential by its hashed key (`AgentCredential` entity ID).
    pub fn invalidate_key_hash(&self, key_hash: &str) {
        let mut cache = self.cache.write().unwrap(); // ci-ok: infallible lock
        let suffix = format!(":{key_hash}");
        let keys: Vec<String> = cache
            .keys()
            .filter(|key| key.ends_with(&suffix))
            .cloned()
            .collect();
        for key in keys {
            cache.remove(&key);
        }
    }

    fn get_cached(&self, cache_key: &str) -> Option<ResolvedIdentity> {
        let cache = self.cache.read().unwrap(); // ci-ok: infallible lock
        let entry = cache.get(cache_key)?;
        let now = sim_now();
        if now < entry.expires_at {
            Some(entry.identity.clone())
        } else {
            None
        }
    }

    fn put_cached(&self, cache_key: String, identity: ResolvedIdentity) {
        let expires_at = sim_now() + chrono::Duration::seconds(CACHE_TTL_SECS);
        self.put_cached_until(cache_key, identity, expires_at);
    }

    /// Cache with an explicit expiry (JWT path caps this at the token's `exp`).
    fn put_cached_until(
        &self,
        cache_key: String,
        identity: ResolvedIdentity,
        expires_at: chrono::DateTime<chrono::Utc>,
    ) {
        let mut cache = self.cache.write().unwrap(); // ci-ok: infallible lock

        // Evict expired entries opportunistically (bounded work: max 32 per insert).
        let now = sim_now();
        let expired_keys: Vec<String> = cache
            .iter()
            .filter(|(_, entry)| now >= entry.expires_at)
            .take(32)
            .map(|(k, _)| k.clone())
            .collect();
        for k in expired_keys {
            cache.remove(&k);
        }

        cache.insert(
            cache_key,
            CacheEntry {
                identity,
                expires_at,
            },
        );
    }
}

fn cache_key(tenant: &TenantId, key_hash: &str) -> String {
    format!("{}:{key_hash}", tenant.as_str())
}

/// Heuristic: does this bearer token have JWS compact form
/// (`header.payload.signature`, three non-empty dot-separated segments)?
///
/// Opaque `AgentCredential` tokens are single-segment, so this cleanly
/// separates the two paths without decoding anything.
fn looks_like_jwt(token: &str) -> bool {
    let mut parts = token.split('.');
    matches!(
        (parts.next(), parts.next(), parts.next(), parts.next()),
        (Some(h), Some(p), Some(s), None) if !h.is_empty() && !p.is_empty() && !s.is_empty()
    )
}

/// Hash a bearer token with SHA-256 for credential lookup.
pub fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    let hash_bytes = hasher.finalize();
    hash_bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_token_deterministic() {
        let h1 = hash_token("test-token-123");
        let h2 = hash_token("test-token-123");
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64); // SHA-256 = 64 hex chars
    }

    #[test]
    fn test_hash_token_different_inputs() {
        let h1 = hash_token("token-a");
        let h2 = hash_token("token-b");
        assert_ne!(h1, h2);
    }

    #[test]
    fn jwt_shape_detection_routes_correctly() {
        // JWS compact form → JWT path.
        assert!(looks_like_jwt("eyJhbGciOiJFUzI1NiJ9.eyJpc3MiOiJ4In0.c2ln"));
        // Opaque credential tokens → registry path.
        assert!(!looks_like_jwt("kc_3f2a9b8c7d6e5f4a"));
        assert!(!looks_like_jwt(""));
        // Wrong segment counts or empty segments are not JWTs.
        assert!(!looks_like_jwt("a.b"));
        assert!(!looks_like_jwt("a.b.c.d"));
        assert!(!looks_like_jwt("a..c"));
        assert!(!looks_like_jwt(".b.c"));
        assert!(!looks_like_jwt("a.b."));
    }
}
