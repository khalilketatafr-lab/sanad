-- Device registry (docs/blueprint/01-architecture.md §6).
-- users(id) arrives with passkey accounts; until then user_id is nullable and
-- unconstrained (anonymous sampling devices have no user).
CREATE TABLE devices (
    id              uuid PRIMARY KEY,                    -- UUIDv7, time-ordered
    user_id         uuid,
    dpop_jkt        text NOT NULL UNIQUE,                -- RFC 7638 thumbprint of DeviceKey-Sign
    ecdh_pub        bytea NOT NULL CHECK (octet_length(ecdh_pub) = 65 AND get_byte(ecdh_pub, 0) = 4),
    platform        jsonb NOT NULL DEFAULT '{}'::jsonb,
    drm_robustness  text,
    created_at      timestamptz NOT NULL DEFAULT now(),
    last_seen       timestamptz NOT NULL DEFAULT now(),
    revoked_at      timestamptz,
    CONSTRAINT dpop_jkt_shape CHECK (dpop_jkt ~ '^[A-Za-z0-9_-]{43}$')
);

CREATE INDEX devices_user_idx ON devices (user_id) WHERE user_id IS NOT NULL;
