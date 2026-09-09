-- /v1/market/search measurement (2026-09-06, WSL edda_dev, 100.2M
-- market rows): a commodity's FRESH slice is ~2% of its rows (gold:
-- 8,908 of 404,503 within 48h), but no index served (commodity,
-- fresh) — the planner paid either 404k heap fetches (4.4s) or 129k
-- per-station probes (858ms) for a query whose targets are P50<150ms
-- / P95<600ms. This covering index makes the fresh slice one
-- index-only range read.
--
-- Ops note: IF NOT EXISTS means a large self-hosted database can
-- pre-build it as CREATE INDEX CONCURRENTLY under the same name and
-- this migration will leave it alone; the plain CREATE here blocks
-- writes for the build duration, which is acceptable at deploy size.
CREATE INDEX IF NOT EXISTS market_commodity_fresh_idx
    ON market (commodity_symbol, observed_at DESC)
    INCLUDE (station_id, sell_price, buy_price, demand, supply);
