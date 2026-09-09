-- The EDSM proxy's bookkeeping (item 48 rider, design ledgered
-- 2026-09-06). knowledge_sweeps mirrors the client's edsm_sweeps cell
-- grid server-side: one row per 100-ly cell the fleet has swept, so N
-- clients asking about the same cell cost ONE upstream fetch per
-- staleness window. knowledge_bodies caches EDSM body lookups verbatim
-- (star systems do not change). Neither table records who asked —
-- the surveillance law's shape: the aggregate stores what was LEARNED.
CREATE TABLE IF NOT EXISTS knowledge_sweeps (
    cell TEXT PRIMARY KEY,
    fetched_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    systems INTEGER NOT NULL DEFAULT 0,
    learned INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS knowledge_bodies (
    system_name TEXT PRIMARY KEY,
    fetched_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    body TEXT NOT NULL
);
