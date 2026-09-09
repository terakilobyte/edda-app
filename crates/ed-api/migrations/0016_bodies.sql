-- Bodies, rings, ring hotspots and surface materials: the mining search
-- moves to the server (B.4, maintainer 2026-09-09: "Local search should be
-- limited to journal data, that's it"). The dump hydration already
-- walks every body (the sink's `body` callback kept only the main star
-- for the routing index); these are the rows the local Mining page
-- used to answer from its own galaxy import, in the same shape.
--
-- Only bodies worth a row are stored — landable, or carrying surface
-- materials, rings, or bio/geo signals (the old importer's filter) —
-- so the tables track prospecting, not the whole body census. Newer
-- wins per body on `observed_at` (the record's updateTime, else the
-- system's date); children are rewritten with their body.
--
-- Nothing here names a commander: bodies are astronomy.

CREATE TABLE IF NOT EXISTS bodies (
    id64                BIGINT PRIMARY KEY,
    system_address      BIGINT NOT NULL,
    body_id             INTEGER,
    name                TEXT,
    type                TEXT,
    sub_type            TEXT,
    is_landable         BOOLEAN NOT NULL DEFAULT FALSE,
    distance_to_arrival DOUBLE PRECISION,
    gravity             DOUBLE PRECISION,
    atmosphere          TEXT,
    volcanism           TEXT,
    bio_signals         INTEGER,
    geo_signals         INTEGER,
    observed_at         TIMESTAMPTZ NOT NULL,
    provenance          TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS bodies_system_idx ON bodies (system_address);

CREATE TABLE IF NOT EXISTS body_materials (
    body_id64 BIGINT NOT NULL REFERENCES bodies (id64) ON DELETE CASCADE,
    material  TEXT   NOT NULL,
    percent   DOUBLE PRECISION NOT NULL,
    PRIMARY KEY (body_id64, material)
);
-- "richest first" for a material: (material, percent DESC) is the scan.
CREATE INDEX IF NOT EXISTS body_materials_material_idx ON body_materials (material, percent DESC);

CREATE TABLE IF NOT EXISTS rings (
    body_id64    BIGINT NOT NULL REFERENCES bodies (id64) ON DELETE CASCADE,
    name         TEXT   NOT NULL,
    type         TEXT,
    mass         DOUBLE PRECISION,
    inner_radius DOUBLE PRECISION,
    outer_radius DOUBLE PRECISION,
    PRIMARY KEY (body_id64, name)
);
CREATE INDEX IF NOT EXISTS rings_type_idx ON rings (type);

CREATE TABLE IF NOT EXISTS ring_hotspots (
    body_id64 BIGINT NOT NULL REFERENCES bodies (id64) ON DELETE CASCADE,
    ring_name TEXT   NOT NULL,
    material  TEXT   NOT NULL,
    count     INTEGER NOT NULL,
    PRIMARY KEY (body_id64, ring_name, material)
);
CREATE INDEX IF NOT EXISTS ring_hotspots_material_idx ON ring_hotspots (material);
