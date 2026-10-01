-- Passkey accounts (docs/blueprint/01-architecture.md §3.2, §6).
-- There is no password column anywhere: a user is a set of passkeys.
CREATE TABLE users (
    id           uuid PRIMARY KEY,                       -- UUIDv7
    -- WebAuthn user.id: random, opaque, never the account id or an email.
    user_handle  bytea NOT NULL UNIQUE CHECK (octet_length(user_handle) BETWEEN 16 AND 64),
    created_at   timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE passkeys (
    credential_id    bytea PRIMARY KEY CHECK (octet_length(credential_id) BETWEEN 1 AND 1023),
    user_id          uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    public_key_cose  bytea NOT NULL CHECK (octet_length(public_key_cose) BETWEEN 32 AND 2048),
    alg              smallint NOT NULL CHECK (alg IN (-7, -8, -257)),   -- ES256, EdDSA, RS256
    sign_count       bigint NOT NULL DEFAULT 0 CHECK (sign_count BETWEEN 0 AND 4294967295),
    aaguid           bytea NOT NULL CHECK (octet_length(aaguid) = 16),
    backup_eligible  boolean NOT NULL,
    backed_up        boolean NOT NULL,
    created_at       timestamptz NOT NULL DEFAULT now(),
    last_used_at     timestamptz,
    CONSTRAINT backup_state_requires_eligibility CHECK (backup_eligible OR NOT backed_up)
);

CREATE INDEX passkeys_user_idx ON passkeys (user_id);

-- Devices now belong to users once a passkey signs in on them.
ALTER TABLE devices
    ADD CONSTRAINT devices_user_fk FOREIGN KEY (user_id) REFERENCES users (id);
