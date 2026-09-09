-- Station identity ingest (2026-09-04): pads, carrier flag, arrival
-- distance, type and services from EDDN journal/1 Docked events, and
-- confiscated goods from commodity/3 prohibited[]. Until now the server
-- held no station identity beyond name + system — the EBEX
-- station-details addendum ships what these columns learn.
ALTER TABLE stations ADD COLUMN IF NOT EXISTS pad_small integer;
ALTER TABLE stations ADD COLUMN IF NOT EXISTS pad_medium integer;
ALTER TABLE stations ADD COLUMN IF NOT EXISTS pad_large integer;
ALTER TABLE stations ADD COLUMN IF NOT EXISTS is_carrier boolean NOT NULL DEFAULT false;
ALTER TABLE stations ADD COLUMN IF NOT EXISTS arrival_ls double precision;
ALTER TABLE stations ADD COLUMN IF NOT EXISTS station_type text;
ALTER TABLE stations ADD COLUMN IF NOT EXISTS identity_observed_at timestamptz;

CREATE TABLE IF NOT EXISTS station_prohibited (
    station_id bigint NOT NULL REFERENCES stations(id) ON DELETE CASCADE,
    symbol     text   NOT NULL,
    PRIMARY KEY (station_id, symbol)
);

CREATE TABLE IF NOT EXISTS station_services (
    station_id bigint NOT NULL REFERENCES stations(id) ON DELETE CASCADE,
    service    text   NOT NULL,
    PRIMARY KEY (station_id, service)
);
