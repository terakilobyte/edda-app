#!/usr/bin/env bash
# Weekly publications only. The weekly SYNCS (galaxy_7days from Spansh,
# bodies7days from EDSM) were retired (maintainer, 2026-09-09: "Why are we
# syncing any more than daily? Why do we not trust our own data?"): EDDN
# runs as its own unit and survives a swap, and the nightly galaxy_1day
# carries what EDDN never saw. Break-glass for a missed night is one
# manual line (deploy/README.md, "Break glass").
set -euo pipefail

/usr/local/bin/ed-api publish-community
/usr/local/bin/ed-api publish-stars
