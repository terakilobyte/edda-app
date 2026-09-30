-- 2026-09-29: the shipyard twin of 0022 (2.4M rows, 4.5 s locally).
CREATE INDEX CONCURRENTLY shipyard_ship_symbol_idx ON shipyard (ship_symbol, station_id);
