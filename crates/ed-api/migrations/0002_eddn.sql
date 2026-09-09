ALTER TABLE systems DROP CONSTRAINT IF EXISTS systems_address_check;
ALTER TABLE systems ALTER COLUMN x DROP NOT NULL;
ALTER TABLE systems ALTER COLUMN y DROP NOT NULL;
ALTER TABLE systems ALTER COLUMN z DROP NOT NULL;
ALTER TABLE systems ALTER COLUMN population DROP NOT NULL;
ALTER TABLE systems ALTER COLUMN source_observed_at DROP NOT NULL;
ALTER TABLE systems ADD COLUMN IF NOT EXISTS security TEXT;
ALTER TABLE systems ADD COLUMN IF NOT EXISTS allegiance TEXT;
ALTER TABLE systems ADD COLUMN IF NOT EXISTS controlling_power TEXT;
ALTER TABLE systems ADD COLUMN IF NOT EXISTS power_state TEXT;
ALTER TABLE systems ADD COLUMN IF NOT EXISTS powers TEXT;
ALTER TABLE systems ADD COLUMN IF NOT EXISTS eddn_observed_at TIMESTAMPTZ;

CREATE SEQUENCE IF NOT EXISTS provisional_system_address_seq
    AS BIGINT START WITH -1 INCREMENT BY -1 MINVALUE -9223372036854775808 MAXVALUE -1;

CREATE TABLE IF NOT EXISTS stations (
    id BIGINT PRIMARY KEY,
    system_address BIGINT NOT NULL REFERENCES systems(address),
    name TEXT,
    has_market BOOLEAN NOT NULL DEFAULT false,
    has_outfitting BOOLEAN NOT NULL DEFAULT false,
    has_shipyard BOOLEAN NOT NULL DEFAULT false,
    market_observed_at TIMESTAMPTZ,
    outfitting_observed_at TIMESTAMPTZ,
    shipyard_observed_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS stations_system_idx ON stations (system_address);
CREATE INDEX IF NOT EXISTS stations_name_ci_idx ON stations (system_address, lower(name));

CREATE TABLE IF NOT EXISTS commodities (
    symbol TEXT PRIMARY KEY
);

CREATE TABLE IF NOT EXISTS market (
    station_id BIGINT NOT NULL REFERENCES stations(id) ON DELETE CASCADE,
    commodity_symbol TEXT NOT NULL REFERENCES commodities(symbol),
    buy_price BIGINT NOT NULL,
    sell_price BIGINT NOT NULL,
    demand BIGINT NOT NULL,
    supply BIGINT NOT NULL,
    observed_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (station_id, commodity_symbol)
);

CREATE INDEX IF NOT EXISTS market_sell_idx ON market (commodity_symbol, sell_price DESC);
CREATE INDEX IF NOT EXISTS market_buy_idx ON market (commodity_symbol, buy_price)
    WHERE buy_price > 0 AND supply > 0;

CREATE TABLE IF NOT EXISTS modules (
    symbol TEXT PRIMARY KEY
);

CREATE TABLE IF NOT EXISTS outfitting (
    station_id BIGINT NOT NULL REFERENCES stations(id) ON DELETE CASCADE,
    module_symbol TEXT NOT NULL REFERENCES modules(symbol),
    PRIMARY KEY (station_id, module_symbol)
);

CREATE TABLE IF NOT EXISTS ships (
    symbol TEXT PRIMARY KEY
);

CREATE TABLE IF NOT EXISTS shipyard (
    station_id BIGINT NOT NULL REFERENCES stations(id) ON DELETE CASCADE,
    ship_symbol TEXT NOT NULL REFERENCES ships(symbol),
    PRIMARY KEY (station_id, ship_symbol)
);

CREATE TABLE IF NOT EXISTS eddn_ingestion (
    singleton BOOLEAN PRIMARY KEY DEFAULT true CHECK (singleton),
    received BIGINT NOT NULL DEFAULT 0,
    applied BIGINT NOT NULL DEFAULT 0,
    skipped BIGINT NOT NULL DEFAULT 0,
    errors BIGINT NOT NULL DEFAULT 0,
    last_message_at TIMESTAMPTZ,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

INSERT INTO eddn_ingestion (singleton) VALUES (true) ON CONFLICT DO NOTHING;
