CREATE TABLE IF NOT EXISTS artifact_publications (
    id BIGSERIAL PRIMARY KEY,
    product TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('building', 'complete', 'failed')),
    artifact_path TEXT,
    artifact_bytes BIGINT CHECK (artifact_bytes >= 0),
    artifact_sha256 TEXT,
    source_watermark TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    completed_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS artifact_publications_product_idx
    ON artifact_publications (product, id DESC)
    WHERE status = 'complete';
