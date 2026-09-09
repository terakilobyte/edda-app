-- A system the EDDN adapter only knew by name lives under a provisional
-- negative address until a bulk source supplies the real one; moving the
-- row must carry its stations along.
DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint
        WHERE conname = 'stations_system_address_fkey' AND confupdtype = 'c'
    ) THEN
        ALTER TABLE stations DROP CONSTRAINT IF EXISTS stations_system_address_fkey;
        ALTER TABLE stations ADD CONSTRAINT stations_system_address_fkey
            FOREIGN KEY (system_address) REFERENCES systems(address) ON UPDATE CASCADE;
    END IF;
END $$;
