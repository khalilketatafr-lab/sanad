-- Reading sessions (docs/blueprint/05-security-and-drm.md §5).
-- One active reading session per device. Holds the 64-bit session id that
-- seeds the Ex Libris watermark, and the Sentinel risk state (level + the
-- level-1 persistence streak). No content, ever.
CREATE TABLE sessions (
    device_id   uuid PRIMARY KEY REFERENCES devices (id) ON DELETE CASCADE,
    session_id  bigint NOT NULL,                 -- 64-bit, stored as i64 bit-for-bit
    user_id     uuid,
    level       smallint NOT NULL DEFAULT 0 CHECK (level BETWEEN 0 AND 4),
    l1_streak   integer NOT NULL DEFAULT 0 CHECK (l1_streak >= 0),
    created_at  timestamptz NOT NULL DEFAULT now(),
    last_seen   timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX sessions_user_idx ON sessions (user_id) WHERE user_id IS NOT NULL;
