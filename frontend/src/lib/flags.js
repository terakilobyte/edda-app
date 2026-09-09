// Build-time feature flags. A flag is a constant, not a setting: flipping
// one is a commit, so a release either has the feature or does not.
//
// Carrier routing (Item 52 C) is hidden until 0.3.0 (maintainer, 2026-09-07,
// after the 0.2.8 what's-new outage: "let's be methodical"). The backend
// commands, the ship computer tools and their tests stay; only the
// route page's section goes.
export const CARRIER_ROUTING = false;
