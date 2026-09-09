-- Display metadata for the commodity catalog, hydrated from the
-- community-maintained FDevIDs tables (EDCD/FDevIDs, commodity.csv).
-- Empty means not yet hydrated; the publisher encodes empty as absent.
ALTER TABLE commodities ADD COLUMN IF NOT EXISTS name TEXT NOT NULL DEFAULT '';
ALTER TABLE commodities ADD COLUMN IF NOT EXISTS category TEXT NOT NULL DEFAULT '';
