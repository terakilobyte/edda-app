-- Main-star classes learned beyond the bootstrap dump: EDSM bodies dumps,
-- EDSM lookups for systems clients asked about, commanders' scans. One row
-- per system; a newer observation replaces an older one (strictly newer).
CREATE TABLE IF NOT EXISTS stars (
    address     BIGINT PRIMARY KEY CHECK (address > 0),
    class       SMALLINT NOT NULL CHECK (class >= 0 AND class < 16),
    scoopable   BOOLEAN NOT NULL,
    subtype     TEXT NOT NULL,
    source      TEXT NOT NULL,
    observed_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX IF NOT EXISTS stars_observed_idx ON stars (observed_at DESC);

-- Systems a client asked about that the store could not answer: the
-- lookup queue the server drains against EDSM, one request at a time.
CREATE TABLE IF NOT EXISTS star_lookups (
    address      BIGINT PRIMARY KEY CHECK (address > 0),
    requested_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    attempts     INTEGER NOT NULL DEFAULT 0,
    last_error   TEXT
);
