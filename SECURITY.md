# Security policy

## Supported versions

Only the latest release is supported. The app updates itself from
`https://api.edda-app.com`; if you are on an older version, update before
reporting.

## Reporting a vulnerability

Please do not open a public issue for a security problem. Report it
privately through GitHub's private vulnerability reporting on this
repository ("Report a vulnerability" under the Security tab), which opens
a draft security advisory with the maintainer, `@terakilobyte`. Include
the version, the platform, what you observed and how to reproduce it.

## Scope

- The desktop app, including its auto-updater and the signed installers
  it downloads.
- The server API at `api.edda-app.com` and the artifacts it publishes.
- The website at `edda-app.com`, including the browser route planner.
- Anything in this repository that could expose a commander's identity or
  position: that is a security issue here, not a privacy nicety.

## Out of scope

- Frontier Developments' services (the game, the companion API, their
  websites).
- Third-party data providers (Spansh, EDSM, EDDN) and their
  infrastructure.
- Vulnerabilities that require a compromised machine or a modified build.

## Response and disclosure

Best effort, by one person: expect an acknowledgement within a week, and a
fix timeline that depends on severity. We prefer coordinated disclosure:
give us a chance to ship a fix and let the auto-updater reach most users
before details go public. We credit reporters in the advisory and the
release notes unless asked not to.
