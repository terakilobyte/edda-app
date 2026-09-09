-- What a publication build costs (measurement doctrine rule 2: the
-- delta ships daily, so its price is observable daily). The building
-- process is a short-lived CLI, not the serve process that owns
-- /metrics — so the numbers land here and serve re-emits the latest
-- complete row per product as gauges.
ALTER TABLE artifact_publications
    ADD COLUMN IF NOT EXISTS rows_published BIGINT,
    ADD COLUMN IF NOT EXISTS cpu_seconds DOUBLE PRECISION,
    ADD COLUMN IF NOT EXISTS phase_seconds JSONB;
