-- Surface mining locations per body (2026-09-19). The DSS reports them in
-- SAASignalsFound as {"Type":"$PlanetaryMiningLocation_Name;","Count":N},
-- EDDN carries the event, and the ingest dropped the signal along with
-- the bio/geo keys it never meant to keep. Sized on the box before adding:
-- ~39,400 body-signal rows and ~10,200 ring hotspots a day reach us
-- (docs/benches/2026-09-19-eddn-body-signals-rate.csv); this is a fraction
-- of the former. What a location yields is not on EDDN at all
-- (MiningRefined is not in the journal/1 allowlist), so this column says
-- WHERE, and the community survey says WHAT.
ALTER TABLE bodies ADD COLUMN IF NOT EXISTS mining_locations INTEGER;
CREATE INDEX IF NOT EXISTS bodies_mining_locations_idx
    ON bodies (system_address) WHERE mining_locations > 0;
