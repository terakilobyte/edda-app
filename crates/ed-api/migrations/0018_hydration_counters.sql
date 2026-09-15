-- Per-kind hydration counters (2026-09-15).
--
-- `rows_applied` was written as the systems count alone, so a run that
-- applied nothing but stations recorded zero: the 2026-09-15 stations
-- backfill logged rows_applied = 0 while applying 799,068 identities,
-- 1,242,732 bodies, 294,666 hotspots and 2,191 star classes. Anything
-- read from that column — including the feed-vs-dump question, which
-- asks how much the dump still teaches week over week — was measuring
-- one category and calling it the whole.
--
-- The summary line already carried every count; only the row did not.
-- These columns persist what the log prints, so the curve can be drawn
-- from the database instead of scraped from journald.
--
-- `rows_applied` now means ROWS WRITTEN: systems + identities + market
-- rows + stars + bodies + hotspots. Boards (`snapshots_applied`) are
-- containers for market rows and would double-count, so they are
-- recorded but not summed.
ALTER TABLE service_hydrations ADD COLUMN IF NOT EXISTS systems_seen BIGINT NOT NULL DEFAULT 0;
ALTER TABLE service_hydrations ADD COLUMN IF NOT EXISTS systems_applied BIGINT NOT NULL DEFAULT 0;
ALTER TABLE service_hydrations ADD COLUMN IF NOT EXISTS systems_unfiled BIGINT NOT NULL DEFAULT 0;
ALTER TABLE service_hydrations ADD COLUMN IF NOT EXISTS stations_seen BIGINT NOT NULL DEFAULT 0;
ALTER TABLE service_hydrations ADD COLUMN IF NOT EXISTS snapshots_applied BIGINT NOT NULL DEFAULT 0;
ALTER TABLE service_hydrations ADD COLUMN IF NOT EXISTS snapshots_skipped BIGINT NOT NULL DEFAULT 0;
ALTER TABLE service_hydrations ADD COLUMN IF NOT EXISTS identities_applied BIGINT NOT NULL DEFAULT 0;
ALTER TABLE service_hydrations ADD COLUMN IF NOT EXISTS identities_skipped BIGINT NOT NULL DEFAULT 0;
ALTER TABLE service_hydrations ADD COLUMN IF NOT EXISTS market_rows BIGINT NOT NULL DEFAULT 0;
ALTER TABLE service_hydrations ADD COLUMN IF NOT EXISTS stars_taught BIGINT NOT NULL DEFAULT 0;
ALTER TABLE service_hydrations ADD COLUMN IF NOT EXISTS bodies_applied BIGINT NOT NULL DEFAULT 0;
ALTER TABLE service_hydrations ADD COLUMN IF NOT EXISTS hotspots_applied BIGINT NOT NULL DEFAULT 0;
ALTER TABLE service_hydrations ADD COLUMN IF NOT EXISTS parse_errors BIGINT NOT NULL DEFAULT 0;
