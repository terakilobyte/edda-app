# EDDA server deploy kit

Target: one one 4-8 vCPU / 16 GB Ubuntu 24.04 x86_64 box at
`api.edda-app.com`. Everything the box runs is
declared here; nothing is configured by hand over SSH. The measurements
behind the sizing live in the pre-release engineering notes ("SERVER SIZING
CLOSED", 2026-09-04: Postgres 32 GB near-steady-state, ingest ~630k
msgs/day at 0.4% core, first installs ~11 GB egress each).

## Layout on the box

| path                        | what                                        |
|-----------------------------|---------------------------------------------|
| /usr/local/bin/ed-api       | the server binary (release.yml, `apply api`) |
| /etc/edda/api.env           | DATABASE_URL, artifact dir, bind (0600)     |
| /var/lib/edda-deploy/inbox  | where CI's rsync lands (user deploy only)   |
| /usr/local/bin/edda-deploy  | the deploy key's forced command (allowlist) |
| /usr/local/sbin/edda-apply  | the root half: four verbs, named in sudoers |
| /var/lib/edda/artifacts     | manifest + products + app/ (self-update)    |
| /var/lib/edda/dumps         | weekly upstream dumps (transient)           |
| /var/lib/edda/backups       | nightly irreplaceables pg_dump (14 kept)    |
| /etc/caddy/Caddyfile        | TLS + reverse proxy to 127.0.0.1:8787       |

## First bring-up

1. DNS at Cloudflare: `A api -> <box IPv4>`, `AAAA api ->
   <box IPv6>`, both DNS-only (grey cloud), done.
2. From this repo on the dev machine:
   `.\deploy\push-api.ps1 -Bootstrap` — builds ed-api in WSL, copies the
   binary and this kit to the box, runs `bootstrap.sh` (packages,
   firewall, postgres, caddy, units, timers), starts the service.
3. Seed the database (on the box, as edda; hours, run in tmux):
   `ed-api hydrate --fdev-ids` then
   `ed-api hydrate --spansh /var/lib/edda/dumps/galaxy_stations.json.gz`
   (download from Spansh first; ~1-2 h import at WSL-measured rates).
4. First publishes: `ed-api publish-community && ed-api publish-stars &&
   ed-api publish-market-daily`, then `ed-api build-routing <galaxy
   dump>` when routing should be served (11 GB product; can wait).
5. Point a dev client's Settings -> Community API at
   `https://api.edda-app.com` and watch a sync.

After EVERY deploy or hydrate that rebuilds an index (runbook step,
2026-09-06): run `ANALYZE market; ANALYZE systems; ANALYZE stations;`
in psql as edda. Plan quality on the fresh 100M-row market index
depended on it in the prod cutover — cold trade searches went from
22.9 s to 13.3 s after ANALYZE and nothing else.

## Publish the website (edda-app.com)

The site is static files under `site/`, served by Caddy from
`/var/www/edda-site` (deploy/Caddyfile). It publishes itself:
`.github/workflows/site.yml` runs on every merge to `main` that touches
`site/` (and on demand from the Actions tab), rsyncs to the deploy inbox,
runs `apply site`, and then checks from the outside that the served
front page is byte-for-byte the merged one. A release publishes the
site too (release.yml), together with the notes feed.

Break glass, from a machine with root on the box:

    rsync -av --delete --exclude '*.py' --exclude README.md site/ root@api.edda-app.com:/var/www/edda-site/

2026-09-07 lesson, still the reason the release publishes the site in
the same run as the manifest: 0.2.6 announced itself in the app while
the site still said 0.2.5, and the maintainer read it as "release notes
are broken".

## Cron (systemd timers)

- daily 00:05 UTC: fetch `galaxy_1day` (Spansh, ~1.35 GB) and hydrate it
  (systems, stations, pads, services, boards), then `reconcile-routing` +
  `publish-market-daily` (`edda-daily.sh`; maintainer, 2026-09-08)
- weekly Sun 01:00 UTC: `publish-community` + `publish-stars` only. The
  weekly syncs (`galaxy_7days`, EDSM `bodies7days`) were retired
  2026-09-09 (maintainer: "Why are we syncing any more than daily?") — EDDN
  is its own unit and survives a swap; the nightly carries the rest.
- no scheduled full routing rebuild: `build-routing` is on demand, when a
  measured diff of the applied index against a fresh dump says so.

### Break glass (a missed night, an EDDN outage, a bad hydrate)

```
cd /var/lib/edda/dumps
curl -fsSL -o galaxy_7days.json.gz https://downloads.spansh.co.uk/galaxy_7days.json.gz
ed-api hydrate --spansh galaxy_7days.json.gz     # ~18 min; newer-wins, safe to repeat
ed-api reconcile-routing                         # fold what it taught into the index
rm -f galaxy_7days.json.gz
```

`service_hydrations` says whether a night was missed (one `complete`
row per day from `spansh:galaxy_1day`) and what each run applied vs
what the stored copy outranked — that ratio is the trust measurement.
- nightly 02:00 UTC: pg_dump of the irreplaceable tables (identity,
  prohibited, services, watermarks), gzip, keep 14

## Release = push a tag (CI/CD, maintainer-ruled 2026-09-07: "Never again")

`.github/workflows/ci.yml` runs the Linux tests on every push (docs,
site and docs-only pushes skip; a newer push cancels the run in
flight). `.github/workflows/release.yml` runs on a `vX.Y.Z` tag and
does the whole release: verify the tag matches src-tauri/Cargo.toml and
the newest RELEASE-NOTES.md section → tests on Linux AND Windows →
signed Windows installer, Linux AppImage/.deb, ed-api binary → deploy
(ed-api to the box + restart + readyz + ANALYZE; installers, then
notes.json, then latest.json LAST; website; dashboards) → verify from
the outside (feed version, installer bytes, notes feed, readyz) →
GitHub Release with the artifacts.

To release: bump `version` in src-tauri/Cargo.toml, lead
RELEASE-NOTES.md with a `## X.Y.Z` section (one-paragraph blurb first),
commit, then `git tag vX.Y.Z && git push origin vX.Y.Z`. Nothing else.
The manual scripts (scripts/release-app.*, deploy/push-api.ps1) remain
for emergencies only.

Secrets the repository needs (Settings → Secrets → Actions), each read
from 1Password by path — the values are never written down anywhere
else:

| secret                               | 1Password path                                        |
|--------------------------------------|-------------------------------------------------------|
| `DEPLOY_SSH_KEY`                     | `op://Private/EDDA deploy key/private key`            |
| `TAURI_SIGNING_PRIVATE_KEY`          | `op://Private/EDDA updater signing key/private_key`   |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | `op://Private/EDDA updater signing key/password`      |
| `EDDA_CAPI_CLIENT_ID`                | `op://Private/EDDA CAPI client id/password` (0.3.0+)  |

    op read "op://Private/EDDA deploy key/private key?ssh-format=openssh" | gh secret set DEPLOY_SSH_KEY --repo terakilobyte/edda-app

`?ssh-format=openssh` is not optional: without it `op read` emits the
key as PKCS#8, which OpenSSH cannot load, and the box answers
"Permission denied (publickey)" (first Deploy check, 2026-09-09).

Variables (Settings → Variables → Actions): `DEPLOY_HOST`
(api.edda-app.com) and `DEPLOY_HOST_KEY`, the box's `ssh-ed25519` host
public key, pinned so CI never trusts whoever answers a keyscan:

    ssh-keyscan -t ed25519 api.edda-app.com 2>/dev/null | awk '$2=="ssh-ed25519"{print $2, $3}' | gh variable set DEPLOY_HOST_KEY --repo terakilobyte/edda-app

(compare against your own known_hosts first: `ssh-keygen -F api.edda-app.com`).

Environment `production` on the deploy job: add a required reviewer
there if the plan allows it; otherwise the tag push itself is the
approval, so only collaborators may push tags.

### The deploy key: what it can do, and rotating it

CI logs in as user `deploy`, not root. The key's authorized_keys entry
carries `restrict,command="/usr/local/bin/edda-deploy"`, so no matter
what the runner asks for, the box runs `edda-deploy` and that script
allows exactly: rsync INTO `/var/lib/edda-deploy/inbox` (rrsync,
write-only), `apply api|app|site|dashboards` (sudo to root-owned
`edda-apply`, each verb named exactly in `/etc/sudoers.d/edda-deploy`),
and `status`. Everything else is refused and journaled
(`journalctl -t edda-deploy`). A leaked key can therefore deploy
what CI deploys — nothing more — and the `deploy-check.yml` workflow
proves the refusal on demand.

The key was generated inside 1Password (`op item create --category
"SSH Key" --ssh-generate-key ed25519`); the private half has never been
on a disk. Its public half is committed as `deploy/deploy-key.pub` and
`deploy/install-deploy-user.sh` (idempotent, root, no service restarts)
writes the box's authorized_keys from that file.

To rotate:

1. `op item create --category "SSH Key" --title "EDDA deploy key" --vault Private --ssh-generate-key ed25519`
   (archive the old item after step 4).
2. `op read "op://Private/EDDA deploy key/public key" > deploy/deploy-key.pub`,
   open a pull request, merge.
3. Copy the four files the installer reads to the box and run it (no
   checkout on the box; it holds no source):

       ssh root@api.edda-app.com 'mkdir -p /root/deploy-kit'
       scp deploy/install-deploy-user.sh deploy/deploy-key.pub deploy/edda-deploy deploy/edda-apply root@api.edda-app.com:/root/deploy-kit/
       ssh root@api.edda-app.com 'bash /root/deploy-kit/install-deploy-user.sh'
4. Set the secret (command above), then run **Deploy check** from the
   Actions tab; it must connect, report readyz, and be refused `id`.

The old CI key lived in root's authorized_keys; `install-deploy-user.sh`
lists root's keys at the end so it can be deleted by hand — only the
maintainer's own key belongs there.

Release notes on the website are a FEED: `site/build-notes.py --out
notes.json` from RELEASE-NOTES.md, published at
/v1/app/notes.json; site/notes/index.html fetches it. No page is
regenerated by hand.

## Release the app to the world (manual, emergencies only)

**The version law** (maintainer, 2026-09-05, after three releases shipped
while main swore 0.2.0): `src-tauri/Cargo.toml` carries THE version —
tauri.conf.json has no version field and inherits it, and both release
scripts read it and refuse a dirty src-tauri. Releasing = bump the
toml, commit, run the script. No stamping anywhere: a build's version
is simply true, and `env!("CARGO_PKG_VERSION")` (telemetry, feedback)
cannot lie by construction.

`.\scripts\release-app.ps1 -Version x.y.z -Notes "..." -ApiBase
https://api.edda-app.com -ArtifactDir <scp target or mounted path>` —
or publish locally and rsync `app/` up. Before ANY build reaches
another human: `DEFAULT_COMMUNITY_API` (src-tauri/src/exchange.rs) and
the updater endpoint (src-tauri/tauri.conf.json) must both say
`https://api.edda-app.com` — that value is baked into installs forever.

## Monitoring (deploy/monitoring/, maintainer make-it-so 2026-09-05)

VictoriaMetrics (127.0.0.1:8428, scrapes ed-api /metrics, 180 d
retention) + Grafana OSS (127.0.0.1:3000) — BOTH loopback-only: the
surveillance law's corollary is that dashboards are never exposed.
Reach them through an SSH tunnel to the host (`ssh -L 3000:127.0.0.1:3000
<host>`), browse http://localhost:3000; the admin credential is
generated by `setup-monitoring.sh` on first run. Dashboards are provisioned from
/var/lib/edda/dashboards (edit in deploy/monitoring/dashboards/, push,
re-run setup). The feedback browse reads Postgres through grafana_ro
(SELECT on feedback only). Setup and every config file live in
deploy/monitoring/; `setup-monitoring.sh` is idempotent. The
/v1/telemetry client ingest lands once its wire contract is
fixed (review proposes; allowlist enforced at the API boundary).

## Hardening baseline (bootstrap does all of it), by threat

- Network: ufw default-deny; only 22/80/443 exist. Postgres and ed-api
  bind loopback only — TLS via Caddy is the sole public door, and it
  404s /metrics (scrape from the box; operational detail is not public).
- SSH: keys only (password + keyboard-interactive off, root
  prohibit-password), fail2ban on top for log hygiene. CI holds no root
  key: it logs in as `deploy`, whose only key is bound to a forced
  command with a closed allowlist (see "The deploy key" above), and the
  runner pins the box's host key instead of scanning for it.
- Patching: unattended security upgrades with auto-reboot 04:30 UTC —
  the service units Restart=always through it.
- Process containment: every unit runs as the unprivileged `edda` user
  under systemd sandboxing — the API additionally with an empty
  capability set, @system-service syscall filter, no exec-writable
  memory, kernel surfaces read-protected, and only /var/lib/edda
  writable. A compromised process can serve wrong bytes; it cannot
  install anything, persist outside its state dir, or reach the kernel.
- Secrets: the box holds ONE secret (the DB password, 0600 root-owned,
  loopback-only anyway). The update-signing key never touches the
  server — a fully compromised server still cannot push code to
  installed apps, only refuse updates; the client verifies signatures
  against the pubkey baked into the binary.
- Supply chain: clients verify every artifact by manifest digest, and
  app updates by minisign signature. The server is a dumb file host
  with a database; trust lives in the client.

Everything on the box is rebuildable from the dumps and the feed; the
nightly pg_dump covers the irreplaceable tables. Per-address rate
limits are enforced by ed-api itself (`deploy/README.md`, "ed-api
under load"); a provider-level backup and an off-box copy of the
nightly dump are the operator's call for a production deployment.

## ed-api under load: the pool, the gates, the lanes (2026-09-07 bench series)

What the box does when many commanders plot and search at once, and the
knobs that exist. All numbers from `docs/benches/` (api-load-rotate,
plot-gate-sweep) and docs/ROADMAP.md; re-measure before changing a default.

- **Planner pool.** `serve` sizes the rayon pool to cores − 2 and nices
  every planner thread +10 (Linux). Before this the default pool took
  every core at normal priority: two fanning plots read 99.5 % CPU and a
  253 ms trade search waited 15 s. Check: `journalctl -u edda-api.service
  | grep "planner pool sized"`, and `/proc/<pid>/task/*/stat` field 19
  for the nice value (a `ps -L` + awk read it wrong once).
- **Two plot lanes** (`plot.rs`): interactive (straight-line ≤ 1,000 ly;
  4 slots, queue 8, 30 s budget) and long (2 slots, queue 4, 120 s). A
  full waiting room answers 503 `queue_full` naming the lane; a budget
  miss answers 504 `budget` with the lane's budget. Slots sum to the
  pool. Env knobs, read at start, production leaves them unset:
  `EDDA_API_PLOT_CONCURRENCY`, `EDDA_API_PLOT_QUEUE` (interactive),
  `EDDA_API_PLOT_LONG_CONCURRENCY`, `EDDA_API_PLOT_LONG_QUEUE`. Measured:
  bubble plots served 435 → 701 → 812 at gate 2/4/6 (refusals 56 → 23 →
  7 %); long plots do not scale with the gate (18–24 served/30 s at
  c=16) and are what the 120 s lane is for.
- **Per-source budgets:** `EDDA_API_ROUTE_RATE_PER_HOUR`,
  `EDDA_API_MARKET_RATE_PER_HOUR`, `EDDA_API_KNOWLEDGE_RATE_PER_HOUR`
  (2,500 default), `EDDA_API_NAMES_RATE_PER_MINUTE` (600). A bench sets
  them in a drop-in (`/etc/systemd/system/edda-api.service.d/*.conf`) and
  removes it after; every bench row must be read with its status codes —
  a 429 or 503 is a fast response and makes p50 look good.
- **Dashboard rows** (`edda-server.json`): Route API — requests by
  outcome, latency p50/p95/p99, cache hit/miss, and the lane row
  (requests by lane+outcome, p95 by lane). Trade search, market search,
  knowledge, station board each have their own row.
- **Postgres instruments:** `pg_stat_statements` is preloaded
  (`conf.d/90-edda-stats.conf`, track = top) with
  `pg_stat_statements.track_planning = on` (ALTER SYSTEM). `plans =
  calls` proves a statement plans per execution; `plan_time 0` proves
  nothing unless track_planning is on. Attribution query:
  `SELECT calls, mean_exec_time, max_exec_time, mean_plan_time, plans,
  left(query,80) FROM pg_stat_statements ORDER BY mean_exec_time DESC`.

### Swapping the binary (the pattern used all night)

```
cd /usr/local/bin
sha256sum ed-api.new                      # compare with the builder's hash BEFORE anything moves
cp -p ed-api ed-api.prev                  # the rollback target is always the binary that just passed
chmod 755 ed-api.new && mv ed-api.new ed-api
systemctl restart edda-api.service
for i in $(seq 1 60); do curl -sf http://127.0.0.1:8787/readyz >/dev/null && break; sleep 1; done
journalctl -u edda-api.service --since "2 min ago" | grep -E "planner pool sized|plot gates sized"
```

Rollback: `cp -p ed-api.prev ed-api && systemctl restart edda-api.service`.
A restart is ~45–60 s of `readyz` false (the routing index maps), during
which the website's version pill and download buttons show "version check
unavailable" — swap when it is quiet. A migration that rewrites a table
(0015 backfilled 830k station rows) makes it ~80 s. Health endpoints are
unversioned: `/readyz`, `/healthz` — `/v1/readyz` is a 404.

### The EDDN feed as its own unit (edda-eddn.service; cutover once, 2026-09-09)

Why: a serve restart lost ~65 s of feed (~250–300 boards) because the
feed lived in the same process and EDDN has no replay. `ed-api ingest`
is the same decode → writer → Postgres path under its own unit; Postgres
is the handoff, there is no socket. Cutover, in this order, with a
binary that knows `ingest` already in place:

```
install -m 644 edda-eddn.service /etc/systemd/system/ && systemctl daemon-reload
systemctl enable --now edda-eddn.service
for i in $(seq 1 60); do curl -sf http://127.0.0.1:8788/readyz >/dev/null && break; sleep 1; done
#   both processes now ingest; the writer is newer-wins per station, duplicates are idempotent
echo 'EDDA_API_EDDN_IN_SERVE=false' >> /etc/edda/api.env
systemctl restart edda-api.service          # the usual ~60 s; the feed does not blink
journalctl -u edda-api.service --since "2 min ago" | grep 'serve: EDDN feed'   # expect eddn="external"
curl -s http://127.0.0.1:8787/readyz | grep -o '"eddn":"[a-z]*"'                # still "receiving"
```

Add the `edda-eddn` job to `vm-scrape.yml` (target 127.0.0.1:8788) so the
`edda_eddn_*` series keep a home. From then on a serve swap is the
section above with no freshness cost; an ingest swap (`systemctl restart
edda-eddn.service`) costs only the ZMQ reconnect — measure it the first
time (`edda_eddn_last_apply_unix_seconds` gap, pre-registered < 5 s).
Rollback: remove the env line, restart serve, `systemctl disable --now
edda-eddn.service`.

Two field notes from the first run (2026-09-09 14:13Z): Caddy's admin
API is off on the box, so `systemctl reload caddy` cannot push a new
Caddyfile - validate, then `systemctl restart caddy` (a two-second
blip). And a site upload that lands via `scp -r` from Windows creates
root-only directories (`drwx------`), which Caddy serves as 403: run
`chmod -R a+rX /var/www/edda-site` after any copy that is not rsync
from a Unix host.
