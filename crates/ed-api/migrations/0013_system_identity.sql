-- The by-name system proxy (/v1/knowledge/system) exists so a commander's
-- Galaxy-tab lookup never touches edsm.net from their own machine (maintainer,
-- 2026-09-06: "route the EDSM calls through our API"). For that proxy to
-- be worth having, an answer fetched upstream once must be answerable
-- LOCALLY forever after -- otherwise every client pays the round trip and
-- EDSM sees the fleet anyway, which is the thing the sphere proxy already
-- fixed for sweeps.
--
-- `systems` already carries allegiance, security and population. These are
-- the two fields the client's EdsmInformation parses that had nowhere to
-- live, so a learned system came back thinner than the upstream answer it
-- was learned from.
ALTER TABLE systems ADD COLUMN IF NOT EXISTS government TEXT;
ALTER TABLE systems ADD COLUMN IF NOT EXISTS economy TEXT;
