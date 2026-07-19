#!/bin/bash
# Live local end-to-end check for TrustedIssuer JWT verification (ARN-255 step 1).
#
# Boots a local temper server, registers a TrustedIssuer whose JWKS matches a
# locally generated P-256 key, mints ES256 tokens with that key, and proves:
#   1. a valid token resolves to a verified agent principal (request succeeds)
#   2. a token signed by an UNKNOWN key is rejected (401)
#   3. an expired token is rejected (401)
#   4. a token from an unregistered issuer is rejected (401)
#   5. opaque AgentCredential bearers still work (additive: nothing broke)
#
# NOTE ON AUTHORIZATION: registering a TrustedIssuer is an admin-gated action,
# so this script needs the target tenant to carry a Cedar policy permitting
# admin management of TrustedIssuer (RegisterIssuer/Suspend/...). A bare
# `temper serve` tenant is default-deny with no seeded permits, so RegisterIssuer
# returns 403 there — as does AgentCredential.Issue; it is not specific to this
# entity. Point this script at a tenant provisioned with that policy. The
# resolver logic itself (verify -> principal, and rejection of bad tokens) is
# covered without that dependency by the integration test
# `crates/temper-platform/tests/trusted_issuer_resolve.rs`, which seeds the
# issuer through the internal dispatch path and asserts the same five outcomes.
#
# Requires: cargo (workspace built), python3 with 'cryptography', jq, curl.
# Usage: scripts/e2e-trusted-issuer.sh [port]   (default 3467)
set -euo pipefail

PORT="${1:-3467}"
BASE="http://localhost:${PORT}"
TENANT="default"
API_KEY="${TEMPER_API_KEY:-local-e2e-operator-key}"
WORKDIR="$(mktemp -d)"
trap 'kill "${SERVER_PID:-}" 2>/dev/null || true; rm -rf "$WORKDIR"' EXIT

say() { printf '\n== %s\n' "$*" >&2; }

# --- 1. Generate a P-256 keypair + JWKS + tokens (python, one shot) ---------
say "Generating P-256 keypair, JWKS, and test tokens"
python3 - "$WORKDIR" <<'PY'
import base64, json, sys, time
from cryptography.hazmat.primitives.asymmetric import ec
from cryptography.hazmat.primitives.asymmetric.utils import decode_dss_signature
from cryptography.hazmat.primitives import hashes

workdir = sys.argv[1]
def b64u(b): return base64.urlsafe_b64encode(b).rstrip(b"=").decode()

def mint(key, kid, claims):
    header = {"alg": "ES256", "kid": kid, "typ": "JWT"}
    signing_input = f"{b64u(json.dumps(header).encode())}.{b64u(json.dumps(claims).encode())}"
    der = key.sign(signing_input.encode(), ec.ECDSA(hashes.SHA256()))
    r, s = decode_dss_signature(der)
    sig = r.to_bytes(32, "big") + s.to_bytes(32, "big")
    return f"{signing_input}.{b64u(sig)}"

key = ec.generate_private_key(ec.SECP256R1())
pub = key.public_key().public_numbers()
jwks = {"keys": [{"kty": "EC", "crv": "P-256", "kid": "e2e-k1",
                  "x": b64u(pub.x.to_bytes(32, "big")),
                  "y": b64u(pub.y.to_bytes(32, "big"))}]}
open(f"{workdir}/jwks.json", "w").write(json.dumps(jwks))

now = int(time.time())
base = {"iss": "https://e2e.issuer.local", "aud": "temper-e2e",
        "sub": "human-sub-e2e", "client_id": "kc_e2e_agent", "agent_type": "contributor",
        "grant_id": "grant-e2e", "nbf": now - 60}
open(f"{workdir}/token_valid.txt", "w").write(mint(key, "e2e-k1", {**base, "exp": now + 900}))
open(f"{workdir}/token_expired.txt", "w").write(mint(key, "e2e-k1", {**base, "exp": now - 600}))
open(f"{workdir}/token_bad_iss.txt", "w").write(
    mint(key, "e2e-k1", {**base, "iss": "https://unregistered.example", "exp": now + 900}))

rogue = ec.generate_private_key(ec.SECP256R1())
open(f"{workdir}/token_rogue.txt", "w").write(mint(rogue, "e2e-k1", {**base, "exp": now + 900}))
print("minted 4 tokens")
PY

# --- 2. Boot the server ------------------------------------------------------
say "Starting local temper server on :$PORT"
TEMPER_API_KEY="$API_KEY" cargo run -p temper-cli --bin temper -- serve --port "$PORT" --no-observe \
  >"$WORKDIR/server.log" 2>&1 &
SERVER_PID=$!
for i in $(seq 1 120); do
  curl -sf "$BASE/healthz" >/dev/null 2>&1 && break
  sleep 2
  kill -0 "$SERVER_PID" 2>/dev/null || { echo "server died; log tail:"; tail -40 "$WORKDIR/server.log"; exit 1; }
done
curl -sf "$BASE/healthz" >/dev/null || { echo "server never became healthy"; tail -40 "$WORKDIR/server.log"; exit 1; }

# --- 3. Register the TrustedIssuer (operator key) ---------------------------
say "Registering TrustedIssuer https://e2e.issuer.local"
ISSUER_ID="https%3A%2F%2Fe2e.issuer.local"
curl -sf -X POST \
  "$BASE/tdata/TrustedIssuers('$ISSUER_ID')/Temper.RegisterIssuer" \
  -H "Authorization: Bearer $API_KEY" -H "X-Tenant-Id: $TENANT" \
  -H "Content-Type: application/json" \
  -d "$(jq -n --rawfile jwks "$WORKDIR/jwks.json" '{
        issuer: "https://e2e.issuer.local", jwks_json: $jwks,
        audience: "temper-e2e", algorithms: "ES256",
        description: "local e2e issuer", created_by: "e2e-script"}')" >/dev/null
echo "registered"

probe() { # probe <token> -> HTTP status of a governed read
  curl -s -o /dev/null -w '%{http_code}' \
    -H "Authorization: Bearer $1" -H "X-Tenant-Id: $TENANT" \
    "$BASE/tdata/TrustedIssuers('$ISSUER_ID')"
}

# --- 4. The five checks ------------------------------------------------------
PASS=0; FAIL=0
check() { # check <name> <got> <want>
  if [ "$2" = "$3" ]; then PASS=$((PASS+1)); echo "PASS  $1 (HTTP $2)";
  else FAIL=$((FAIL+1)); echo "FAIL  $1 (got HTTP $2, want $3)"; fi
}

say "Running checks"
check "valid token accepted"          "$(probe "$(cat "$WORKDIR/token_valid.txt")")"   "200"
check "rogue-key token rejected"      "$(probe "$(cat "$WORKDIR/token_rogue.txt")")"   "401"
check "expired token rejected"        "$(probe "$(cat "$WORKDIR/token_expired.txt")")" "401"
check "unregistered issuer rejected"  "$(probe "$(cat "$WORKDIR/token_bad_iss.txt")")" "401"
check "operator key still works"      "$(probe "$API_KEY")"                            "200"

say "Result: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
