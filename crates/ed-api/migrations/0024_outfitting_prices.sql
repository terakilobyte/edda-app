-- 2026-09-29: outfitting/3 boards carry a price in credits and a price in
-- merc coins per module, and a pre-engineered merc-coin variant shares
-- the plain module's symbol (Balanced Power Distributor is
-- int_powerdistributor_size5_class5 for 500 merc coins). Kept per
-- (station, symbol): the cheapest credit price (0 = not for credits), the
-- cheapest merc-coin price (0 = none), and the merc entries' FDev ids.
-- NULL = a v2 board, unknown. Nullable, no default: instant on 66M rows.
ALTER TABLE outfitting
    ADD COLUMN IF NOT EXISTS credits_price BIGINT,
    ADD COLUMN IF NOT EXISTS merc_price BIGINT,
    ADD COLUMN IF NOT EXISTS merc_variant_ids TEXT;
