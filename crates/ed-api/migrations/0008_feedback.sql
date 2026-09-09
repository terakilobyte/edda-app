-- Anonymous problem reports from the app (wire contract, ledger
-- 2026-09-05): version, OS, the commander's words, an optional log
-- tail behind explicit client-side consent. NO identity columns by
-- design — the privacy law (CLAUDE.md) forbids them, and rate limiting
-- is in-memory, never persisted.
CREATE TABLE IF NOT EXISTS feedback (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    received_at timestamptz NOT NULL DEFAULT now(),
    version text NOT NULL,
    os text NOT NULL,
    body text NOT NULL,
    log_tail text,
    client_created_at text
);
