# Spike S4: Kernel skeleton and DPoP device binding

**Code:** `crates/kernel` (Rust) · `packages/lumen/src/vault` (Vault Worker, TS)

## Flow

```text
Vault Worker (browser)                                        Kernel
──────────────────────                                        ──────
generateKey ECDH P-256  (non-extractable) ┐
generateKey ECDSA P-256 (non-extractable) ┘ persisted as CryptoKey handles in IndexedDB

POST /kernel/v1/devices   DPoP: proof(jwk = Sign.pub, htm, htu, iat, jti)
                          { ecdh_public_jwk }               ──▶ verify proof → no nonce
                                                            ◀── 400 use_dpop_nonce + DPoP-Nonce
POST /kernel/v1/devices   DPoP: proof(…, nonce)             ──▶ verify proof, nonce, jti replay
                                                                validate ECDH point (on-curve, ≠ Sign key)
                                                                store device (jkt, ecdh_pub)
                                                            ◀── 201 { device_id, access_token (PASETO v4.public,
                                                                      cnf.jkt = thumbprint(Sign.pub)), … }
GET  /kernel/v1/devices/self
     Authorization: DPoP <token>
     DPoP: proof(…, nonce, ath = SHA-256(token))            ──▶ token ✓ · proof ✓ · nonce ✓ · jti fresh
                                                                proof.jkt == token.cnf.jkt == device.dpop_jkt
                                                                device not revoked
                                                            ◀── 200 device
```

## Why not the `dpop` crate

The `dpop` crate (v0.1.1) was evaluated and not adopted for the security core:

- It accepts RSA as well as EC proofs. Sanad device keys are always P-256,
  so only ES256 should be accepted.
- Its proof parsing has no `typ: dpop+jwt` check, no check that the JWK
  carries no private key (`d`), and no `jti` replay store. Those would have
  been wrappers around it anyway.
- It depends on `http` 0.2 (Axum 0.8 uses `http` 1.x) and on a release
  candidate of `picky`. That would bring a second crypto stack into the
  Kernel next to `aws-lc-rs`.

`crates/kernel/src/dpop.rs` implements RFC 9449 §4.3 directly on
`aws-lc-rs`. It is about 150 lines. It covers the crate's checks
(signature, `htm`, `htu`, `iat`, `ath`, key binding) plus the four above,
server nonces (§8–9) and RFC-shaped errors for both the authorization-server
and resource-server roles.

## Hardening decisions

| Decision | Reason |
|---|---|
| ES256 only | Device keys are WebCrypto P-256. Fewer algorithms means a smaller attack surface (`alg: none`, HS256 and RS256 are all tested as rejected). |
| Stateless HMAC nonces (time buckets) | Any replica validates any nonce. Rotating the secret invalidates all of them. |
| Nonce checked before `jti` replay | Proofs rejected for a missing nonce never consume replay-cache space |
| Replay cache fails closed at capacity | Returns 503 rather than silently accepting possible replays |
| PASETO implicit assertion `sanad-kernel/access/v1` | Domain separation: other v4.public tokens from the same key (e.g. RCTs) can never act as access tokens |
| ECDH point validated at registration | Invalid-curve attacks are impossible before any key is ever wrapped to the device |
| ECDH key must differ from the signing key | Key-separation hygiene |
| `htu` built from the configured public origin | Never trust `Host` behind a proxy |
| DB `CHECK` constraints on point shape and thumbprint format | Defense in depth behind API validation |
| Literal SQL only | `sqlx` 0.9 rejects dynamically formatted SQL at compile time |

## Tests

| Layer | Count | Highlights |
|---|---|---|
| Rust unit | 10 | RFC 7515 A.3 ES256 vector, RFC 7638 P-256 thumbprint (independent Python vector), off-curve rejection, nonce windows and forgery, replay expiry and fail-closed, token tamper and cross-purpose rejection |
| Rust integration (in-memory) | 10 | Nonce challenges in both roles; replay; stolen token with another registered key; Bearer refusal; a 15-case table of malformed proofs (`alg none/HS256/RS256`, `typ`, private JWK, `htm`, `htu` path and origin, stale and future `iat`, missing and wrong `ath`, `jti`); query-insensitive `htu`; foreign nonce; duplicate DPoP headers; revocation; ECDH validation; idempotent re-registration |
| Rust integration (real Postgres) | 3 | Migrations, idempotency, sticky revocation, DB constraints, full HTTP flow |
| TS end-to-end against the real binary | 6 | Node WebCrypto client; replay; stolen token; Chromium on the reader origin (CORS preflight, exposed `DPoP-Nonce`, IndexedDB-persisted non-extractable keys across reload, idempotent re-registration); CORS refusal of a foreign origin |

The Chromium test caught a browser-only bug that Node hid: calling a stored
`fetch` with the client as `this` throws "Illegal invocation" in browsers.

## Stub lease (roadmap §10.5)

`POST /kernel/v1/editions/{id}:open` (DPoP-protected) returns the first lease
of a reading session, following blueprint 01 §3.3 exactly:

```text
CK[i]      = HKDF(TMK, salt = edition_id ‖ variant, info = "folio/chunk/v1" ‖ i)   Folio's schedule
EK         = fresh ECDH P-256 key pair per lease
KEK        = HKDF-SHA256(ECDH(EK, DeviceKey-ECDH), salt = lease_id, info = "sanad/lease/v1")
wrapped[i] = AES-KW(KEK, CK[i])                                                  RFC 3394, 40 bytes
```

The response carries the window `[start, end)`, the ephemeral public JWK, the
wrapped keys and a **Reading Capability Token**. The RCT is PASETO v4.public
with implicit assertion `sanad-kernel/rct/v1`, bound to the device's DPoP
thumbprint, so it can never act as an access token and vice versa.

These parts are still stubs:
- the catalog: the canary edition under a published test key, development
  only;
- the entitlement: free editions, any registered device;
- the variant vector (all A) and the fixed 3-chunk window.

The cryptography is not a stub. Only `aws-lc-rs` is used, and the KEK is
zeroized on drop.

The Vault Worker (`packages/lumen/src/vault/lease.ts`, `folio.ts`) unwraps the
keys with WebCrypto into **non-extractable, decrypt-only** AES-GCM keys. It
then validates each chunk header rule for rule against the Rust codec, checks
its identity against the lease, and decrypts.

| Test | Proves |
|---|---|
| Rust unit (4) | Device recovers exactly Folio's chunk keys; another device or lease id fails AES-KW's integrity check; fresh ephemeral key per lease; invalid device point refused |
| Rust integration (4) | Full router flow; RCT verifies and is bound to the DPoP key; window clipping; 404 for unknown editions and operations; 401 without a DPoP-bound token |
| TS e2e, Node | Unwrap against the real binary, decrypt chunks sealed by `seal_fixture`, reject tampered and swapped chunks, no key outside the window, another device cannot unwrap |
| TS e2e, Chromium | The same on the reader origin with IndexedDB keys, chunks fetched over HTTP like a CDN; keys report `extractable: false`, usages `["decrypt"]` |

## Passkeys (roadmap §10.5)

`POST /kernel/v1/auth/passkey:begin` / `:finish`. Both are DPoP-protected:
a ceremony always runs on a registered device and ends by **attaching that
device to the account**, with a new access token (`tier: member`, `uid`).

- **Sign up** (`mode: register`) creates an account whose only credential is
  the new passkey. There is no password column anywhere; the WebAuthn user
  handle is 32 random bytes, never an id or an email. A device that is
  already signed in adds a passkey to its account instead.
- **Sign in** (`mode: authenticate`) uses discoverable credentials, so the
  reader never types a username.
- **Ceremony state:** a signed token (PASETO, implicit assertion
  `sanad-kernel/webauthn/v1`, 5 min). It is bound to the device's DPoP
  thumbprint and accepted once. Replicas share no session store.
- **Device rules:** at most 6 active devices per account (blueprint 01 §3.2).
  A device signed in to one account cannot silently join another.

### Why not `webauthn-rs`

`webauthn-rs-core` 0.5.4 depends on `openssl`/`openssl-sys`. That would mean
a second crypto stack and a system C library in a service that otherwise runs
only `aws-lc-rs`. What we need is narrow and fully specified, so
`crates/kernel/src/webauthn.rs` implements it, about 450 lines with tests:

- WebAuthn L3 §7.1 and §7.2 verification;
- ES256, EdDSA and RS256, which covers platform authenticators, passkey
  managers and Windows Hello;
- `none` attestation only, the consumer-passkey norm;
- user verification required.

| Check | Where |
|---|---|
| `clientDataJSON`: type, challenge (constant-time), origin ∈ reader origins, `crossOrigin` false | `verify_client_data` |
| `rpIdHash` = SHA-256(RP ID); RP ID validated at startup to cover every reader origin (§5.1.4.1) | `AuthenticatorData`, `config::rp_id_covers` |
| UP **and** UV flags; BS ⇒ BE; AT exactly when registering; ED extensions skipped by parsing, trailing bytes rejected | `AuthenticatorData::parse` |
| `fmt: "none"` with an empty `attStmt`; anything else refused | `verify_registration` |
| COSE keys: P-256 points validated on the curve, Ed25519 length, RSA ≥ 2048 bits; other algorithms refused | `CoseKey` |
| Signature over `authData ‖ SHA-256(clientDataJSON)` (ASN.1 ECDSA, Ed25519, PKCS#1 v1.5) | `verify_assertion` |
| Counter must increase unless both sides are 0 (synced passkeys) | `verify_assertion` |
| `rawId` = attested credential id; `userHandle` = stored handle | route |

| Test | Proves |
|---|---|
| Rust unit (6), software authenticator | All three algorithms register and sign in; every rule above rejects its violation |
| Rust integration (4) | Sign up on device A, synced sign-in on device B, single-use device-bound ceremonies, phishing origin, tampered signature, account switch refused, 6-device cap |
| Rust on Postgres (1) | The same flow against the migrations; both devices owned by the user in SQL |
| Chromium e2e (1) | Real WebAuthn through Chromium's CTAP2 virtual authenticator. Device A signs up, the credential is copied into a second profile, and device B signs in to the same account with no username |
