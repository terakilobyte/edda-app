-- Fold the display-name symbols 0009 left behind (census 2026-09-27,
-- production: 728 rows, 298 with a spaced symbol, 316 nameless; 294 of
-- the spaced rows had no market row and four were still being fed by
-- one EDDN sender that names goods as the game prints them — "festive
-- gifts" and "low temp. diamonds", five stations each, the newest that
-- morning). A search for "Micro Controllers" hit the nameless
-- "micro controllers" row before the real one and answered nothing.
-- 297 of the 298 match a named canonical row on lower(name); "low temp.
-- diamonds" is Frontier's own short string for lowtemperaturediamond
-- (FDevIDs says "Low Temperature Diamonds") and folds by the alias
-- below. The apply path now folds the same way on the way in
-- (ed_store::postgres, ed_store::market::frontier_commodity_symbol), so
-- this set is frozen; the migration repoints what has market rows,
-- newer-wins, and deletes the rest.

CREATE TEMP TABLE commodity_fold AS
SELECT s.symbol AS variant, COALESCE(a.symbol, c.symbol) AS canonical
FROM commodities s
LEFT JOIN commodities c
       ON c.name <> '' AND lower(c.name) = s.symbol AND c.symbol <> s.symbol
LEFT JOIN (VALUES ('low temp. diamonds', 'lowtemperaturediamond')) AS a(variant, symbol)
       ON a.variant = s.symbol
WHERE (s.symbol LIKE '% %' OR s.symbol LIKE '%-%' OR s.symbol LIKE '%.%')
  AND s.name = ''
  AND COALESCE(a.symbol, c.symbol) IS NOT NULL;

INSERT INTO market (station_id, commodity_symbol, buy_price, sell_price, demand, supply, observed_at)
SELECT DISTINCT ON (m.station_id, f.canonical)
       m.station_id, f.canonical, m.buy_price, m.sell_price, m.demand, m.supply, m.observed_at
FROM market m
JOIN commodity_fold f ON f.variant = m.commodity_symbol
ORDER BY m.station_id, f.canonical, m.observed_at DESC
ON CONFLICT (station_id, commodity_symbol) DO UPDATE SET
    buy_price   = EXCLUDED.buy_price,
    sell_price  = EXCLUDED.sell_price,
    demand      = EXCLUDED.demand,
    supply      = EXCLUDED.supply,
    observed_at = EXCLUDED.observed_at
WHERE market.observed_at < EXCLUDED.observed_at;

DELETE FROM market m USING commodity_fold f WHERE m.commodity_symbol = f.variant;
DELETE FROM commodities c USING commodity_fold f WHERE c.symbol = f.variant;
DROP TABLE commodity_fold;

-- Whatever spaced, hyphened or dotted spelling nothing references goes
-- too (0009's own rule, run again).
DELETE FROM commodities c
WHERE (c.symbol LIKE '% %' OR c.symbol LIKE '%-%' OR c.symbol LIKE '%.%')
  AND c.name = ''
  AND NOT EXISTS (SELECT 1 FROM market m WHERE m.commodity_symbol = c.symbol);
