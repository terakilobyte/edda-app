CREATE TABLE IF NOT EXISTS service_hydrations (
    id BIGSERIAL PRIMARY KEY,
    source TEXT NOT NULL,
    source_observed_at TIMESTAMPTZ NOT NULL,
    source_bytes BIGINT NOT NULL CHECK (source_bytes >= 0),
    status TEXT NOT NULL CHECK (status IN ('running', 'complete', 'failed')),
    rows_applied BIGINT NOT NULL DEFAULT 0 CHECK (rows_applied >= 0),
    started_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    completed_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS service_hydrations_completed_idx
    ON service_hydrations (completed_at DESC)
    WHERE status = 'complete';

CREATE TABLE IF NOT EXISTS systems (
    address BIGINT PRIMARY KEY CHECK (address > 0),
    name TEXT NOT NULL CHECK (name <> ''),
    x DOUBLE PRECISION NOT NULL,
    y DOUBLE PRECISION NOT NULL,
    z DOUBLE PRECISION NOT NULL,
    population BIGINT NOT NULL CHECK (population >= 0),
    source_observed_at TIMESTAMPTZ NOT NULL,
    provenance TEXT NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS systems_name_ci_idx ON systems (lower(name));
CREATE INDEX IF NOT EXISTS systems_coordinates_idx ON systems (x, y, z);
