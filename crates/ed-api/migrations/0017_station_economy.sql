-- Station economy, government and controlling faction (2026-09-12).
-- `GET /v1/stations` has advertised these three fields since it shipped
-- and hard-coded all three to null: the columns never existed. The
-- client types material traders by station economy (Extraction/Refinery
-- = raw, Industrial = manufactured, High Tech/Military = encoded), so
-- every trader row was discarded and the Engineering tab reported none
-- within range. The Spansh dump carries primaryEconomy, government and
-- controllingFaction on every station and the shared parser already
-- deserializes all three (ed-store galaxy::spansh::Station); only the
-- server's hydration dropped them on the floor. EDDN's Docked events
-- carry StationEconomy, so live traffic keeps them fresh between dumps.
ALTER TABLE stations ADD COLUMN IF NOT EXISTS primary_economy text;
ALTER TABLE stations ADD COLUMN IF NOT EXISTS government text;
ALTER TABLE stations ADD COLUMN IF NOT EXISTS controlling_faction text;

-- The trader lookup filters on economy within a radius; the partial
-- index keeps that from scanning stations that have not learned one.
CREATE INDEX IF NOT EXISTS stations_primary_economy_idx
    ON stations (primary_economy)
    WHERE primary_economy IS NOT NULL;
