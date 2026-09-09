-- Fold the fragmented commodity catalog (census 2026-09-05, production:
-- 1085 rows total; 368 '$x_name;' journal-wrapper rows stranding 353
-- market rows; 287 spaced display-spelling rows stranding 0). Inflow was
-- sealed in 0.2.3 (canonical_symbol at client intern + server apply), so
-- this set is frozen; the fold makes search and display see ONE row per
-- good. Variant rows carry no display metadata (measured: every variant
-- name is empty — FDevIDs hydration only ever names canonical rows), so
-- there is nothing to absorb, only rows to repoint and delete.

-- 1. Safety net: every wrapper needs a canonical row to land on.
--    (Measured 0 missing in production; this is for fresh-box replays.)
INSERT INTO commodities (symbol, name, category)
SELECT DISTINCT regexp_replace(lower(symbol), '^\$(.+?)_name;?$', '\1'), '', ''
FROM commodities
WHERE symbol LIKE '$%'
ON CONFLICT (symbol) DO NOTHING;

-- 2. Repoint wrapper market rows, newer-wins. DISTINCT ON guards the
--    one-statement double-hit ("$x_name" and "$x_name;" both feeding the
--    same (station, canonical) key would abort the ON CONFLICT).
INSERT INTO market (station_id, commodity_symbol, buy_price, sell_price, demand, supply, observed_at)
SELECT DISTINCT ON (m.station_id, regexp_replace(lower(m.commodity_symbol), '^\$(.+?)_name;?$', '\1'))
       m.station_id,
       regexp_replace(lower(m.commodity_symbol), '^\$(.+?)_name;?$', '\1'),
       m.buy_price, m.sell_price, m.demand, m.supply, m.observed_at
FROM market m
WHERE m.commodity_symbol LIKE '$%'
ORDER BY m.station_id,
         regexp_replace(lower(m.commodity_symbol), '^\$(.+?)_name;?$', '\1'),
         m.observed_at DESC
ON CONFLICT (station_id, commodity_symbol) DO UPDATE SET
    buy_price   = EXCLUDED.buy_price,
    sell_price  = EXCLUDED.sell_price,
    demand      = EXCLUDED.demand,
    supply      = EXCLUDED.supply,
    observed_at = EXCLUDED.observed_at
WHERE market.observed_at < EXCLUDED.observed_at;

-- 3. Losers out: market rows first (FK), then the wrapper rows, then any
--    spaced display spelling nothing references. A spaced row that ever
--    gains a market reference is deliberately KEPT — this migration
--    deletes only what it can prove is stranded.
DELETE FROM market WHERE commodity_symbol LIKE '$%';
DELETE FROM commodities WHERE symbol LIKE '$%';
DELETE FROM commodities c
WHERE c.symbol LIKE '% %'
  AND NOT EXISTS (SELECT 1 FROM market m WHERE m.commodity_symbol = c.symbol);

-- 4. Hyphened display spellings, same proof-of-strandedness guard
--    (2026-09-06 census, production: helium-3, agri-medicines,
--    auto-fabricators, meta-alloys — all with zero market rows; no
--    canonical FDevIDs symbol contains a hyphen).
DELETE FROM commodities c
WHERE c.symbol LIKE '%-%'
  AND NOT EXISTS (SELECT 1 FROM market m WHERE m.commodity_symbol = c.symbol);
