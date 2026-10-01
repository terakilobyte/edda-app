-- 2026-10-01: the trade panel's "sell to faction" box completes from the
-- server. A DISTINCT over stations.controlling_faction costs ~140 ms per
-- keystroke on the 851k-row table and a btree index made it slower (1 s,
-- measured on the production mirror); 38,911 distinct names fit a table
-- that answers in well under a millisecond. Factions are near-static —
-- new ones arrive with player applications and colonisation, control of
-- stations is what moves — so the writer adds a name the first time a
-- Docked event carries it and never removes one.
CREATE TABLE IF NOT EXISTS factions (
    name TEXT PRIMARY KEY
);
INSERT INTO factions (name)
    SELECT DISTINCT controlling_faction FROM stations WHERE controlling_faction IS NOT NULL
    ON CONFLICT DO NOTHING;
