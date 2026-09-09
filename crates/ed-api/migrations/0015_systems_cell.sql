-- A grid cell the planner can COUNT (maintainer, 2026-09-07: "We can't risk the
-- planner estimate bug on trade, we need to handle that too" and "I
-- essentially want the same station count we got using the market
-- indexes we built locally").
--
-- The searches select stations inside a sphere with three independent
-- BETWEENs on the system's x, y, z. Postgres multiplies three range
-- selectivities and estimates 16 stations for a 40 ly sphere around
-- Deciat that holds 6,655 (measured 2026-09-07, even with a statistics
-- target of 1000 on the coordinates) — a btree on separate axes cannot
-- reason about a box. The local client answers the same question from
-- a cell-bucketed market index that knows exactly how many stations sit
-- in each cell; this gives the server the same thing: a 100 ly cube id
-- (the knowledge grid's CELL_LY) on STATIONS, indexed, with a high
-- statistics target so the most-common-values list carries the bubble's
-- cells with their true counts. `st.cell = ANY(<cells covering the
-- box>)` plus the exact sphere test replaces the BETWEENs, and the
-- planner's row estimate for the sphere becomes the real station count.
--
-- MEASURED (2026-09-07, Deciat / LHS 3447, 40 ly, EXPLAIN ANALYZE cold):
--   predicate                     box estimate vs actual      trade total
--   x/y/z BETWEENs (before)       19 vs 6,655 (350× under)    253 ms
--   st.cell = ANY (100 ly cells)  5,098 vs 6,655 (1.3× under) 820 ms  ← the
--       four bubble-core cells hold 73k stations; all scanned before the
--       sphere test trims them: the count was right and the plan was slow.
--   sy.cell = ANY (100 ly cells)  ~950 vs 6,655 (7× under)     140 ms  ← ships:
--       ~7k systems scanned by cell, stations joined only for the 550 that
--       pass the sphere; estimate two orders of magnitude better than before,
--       plan cheaper than before. stations.cell stays (11 MB, trigger-kept)
--       for a finer grid later if the 7× ever flips a plan.
-- Encoding (mirrored in market_search::cells_covering — keep in step):
--   cx = floor(x / 100), likewise cy, cz; each offset by 1024 (the
--   galaxy spans ±~70,000 ly → ±700 cells); cell = ((cx+1024)*2048 +
--   (cy+1024))*2048 + (cz+1024).

-- 1. The system's cell, generated from its coordinates.
ALTER TABLE systems ADD COLUMN IF NOT EXISTS cell BIGINT GENERATED ALWAYS AS (
    CASE WHEN x IS NULL OR y IS NULL OR z IS NULL THEN NULL
         ELSE ((floor(x / 100.0)::bigint + 1024) * 2048
               + (floor(y / 100.0)::bigint + 1024)) * 2048
              + (floor(z / 100.0)::bigint + 1024)
    END
) STORED;
CREATE INDEX IF NOT EXISTS systems_cell_idx ON systems (cell);

-- 2. The station's cell: a plain column kept equal to its system's cell.
ALTER TABLE stations ADD COLUMN IF NOT EXISTS cell BIGINT;

-- A station row can arrive before its system (EDDN order is not ours to
-- choose), so both directions are covered: a station inserted or moved
-- looks its cell up; a system inserted or re-positioned pushes its cell
-- down to its stations.
CREATE OR REPLACE FUNCTION stations_cell_from_system() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    SELECT cell INTO NEW.cell FROM systems WHERE address = NEW.system_address;
    RETURN NEW;
END $$;
DROP TRIGGER IF EXISTS stations_cell_sync ON stations;
CREATE TRIGGER stations_cell_sync
    BEFORE INSERT OR UPDATE OF system_address ON stations
    FOR EACH ROW EXECUTE FUNCTION stations_cell_from_system();

CREATE OR REPLACE FUNCTION systems_cell_to_stations() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'INSERT' OR NEW.cell IS DISTINCT FROM OLD.cell THEN
        UPDATE stations SET cell = NEW.cell
         WHERE system_address = NEW.address AND cell IS DISTINCT FROM NEW.cell;
    END IF;
    RETURN NULL;
END $$;
DROP TRIGGER IF EXISTS systems_cell_push ON systems;
CREATE TRIGGER systems_cell_push
    AFTER INSERT OR UPDATE OF x, y, z ON systems
    FOR EACH ROW EXECUTE FUNCTION systems_cell_to_stations();

-- 3. Backfill (one pass, ~830k rows), index, statistics that carry the
--    bubble's cells with their true counts.
UPDATE stations st SET cell = sy.cell
  FROM systems sy
 WHERE sy.address = st.system_address AND st.cell IS DISTINCT FROM sy.cell;
CREATE INDEX IF NOT EXISTS stations_cell_idx ON stations (cell);
ALTER TABLE stations ALTER COLUMN cell SET STATISTICS 2000;
ANALYZE systems;
ANALYZE stations;
