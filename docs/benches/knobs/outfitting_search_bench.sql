-- Outfitting search shapes, measured (2026-09-29, after the market tab's
-- module search took 6-7 s and answered nothing for "Bi-Weave Shield
-- Generator"). Three shapes, same origin (Sol) and radius (100 ly):
--   A. what the server runs today: ILIKE '%text%' on module_symbol, where
--      the text is the client's hand-built stem (matches no symbol);
--   B. the same ILIKE with a stem that DOES occur in symbols;
--   C. exact symbols from the client's catalog: module_symbol = ANY($list).
-- Run:  psql -h 127.0.0.1 -p 55432 -d edda_dev -f docs/benches/knobs/outfitting_search_bench.sql
-- On the box the cell filter (sy.cell = ANY(...)) also applies; the local
-- copy predates it, so these are the WITHOUT-cell numbers. Record the
-- verdict in docs/benches/<date>-outfitting-search.csv.
\timing on
\set radius 100
\echo == A. ILIKE, stem that matches nothing (today, Bi-Weave)
EXPLAIN (ANALYZE, BUFFERS, TIMING OFF, SUMMARY ON)
SELECT count(*) FROM outfitting a
  JOIN stations st ON st.id = a.station_id
  JOIN systems sy ON sy.address = st.system_address
 WHERE a.module_symbol ILIKE '%' || 'biweaveshieldgenerator' || '%'
   AND (sy.x-0)^2 + (sy.y-0)^2 + (sy.z-0)^2 <= :radius * :radius;
\echo == B. ILIKE, stem that matches (fuel scoop 5A as the client builds it)
EXPLAIN (ANALYZE, BUFFERS, TIMING OFF, SUMMARY ON)
SELECT count(*) FROM outfitting a
  JOIN stations st ON st.id = a.station_id
  JOIN systems sy ON sy.address = st.system_address
 WHERE a.module_symbol ILIKE '%' || 'fuelscoop_size5_class5' || '%'
   AND (sy.x-0)^2 + (sy.y-0)^2 + (sy.z-0)^2 <= :radius * :radius;
\echo == C. exact symbols from the catalog (all eight Bi-Weave sizes)
EXPLAIN (ANALYZE, BUFFERS, TIMING OFF, SUMMARY ON)
SELECT count(*) FROM outfitting a
  JOIN stations st ON st.id = a.station_id
  JOIN systems sy ON sy.address = st.system_address
 WHERE a.module_symbol = ANY(ARRAY['int_shieldgenerator_size1_class3_fast','int_shieldgenerator_size2_class3_fast','int_shieldgenerator_size3_class3_fast','int_shieldgenerator_size4_class3_fast','int_shieldgenerator_size5_class3_fast','int_shieldgenerator_size6_class3_fast','int_shieldgenerator_size7_class3_fast','int_shieldgenerator_size8_class3_fast'])
   AND (sy.x-0)^2 + (sy.y-0)^2 + (sy.z-0)^2 <= :radius * :radius;
