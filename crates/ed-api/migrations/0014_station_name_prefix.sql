-- Name completion (GET /v1/names/complete?kind=station) answers "stations
-- starting with X" for the whole fleet (RULING, maintainer 2026-09-07: the
-- server knows more than every client; a remote-first install has no
-- station list of its own). The existing stations_name_ci_idx leads with
-- system_address, so a name-only prefix query over ~830k rows is a seq
-- scan without this. text_pattern_ops makes `lower(name) LIKE 'x%'` an
-- index range scan regardless of collation.
CREATE INDEX IF NOT EXISTS stations_name_prefix_idx
    ON stations (lower(name) text_pattern_ops);
