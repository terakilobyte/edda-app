# EDDA observability

The server (`ed-api`) exposes Prometheus text-format metrics at
`GET /metrics`. Emission happens through the [`metrics`](https://metrics.rs)
facade in every crate (`counter!`/`gauge!`/`histogram!` are no-ops
without a recorder, the same contract as `tracing` with no subscriber);
`ed_api::metrics` installs the one recorder per process and owns the
naming conventions — see the module docs in
`crates/ed-api/src/metrics.rs` for the full series table.

Logs are `tracing` with a JSON subscriber (`RUST_LOG` filters; the
default keeps the whole ingest path audible: `ed_api=info,ed_store=info,ed_eddn=info`).

## Storage: VictoriaMetrics

Recommended TSDB: **VictoriaMetrics** (single binary, faster and far
lighter on RAM/disk than Prometheus, PromQL-compatible for Grafana).
The `/metrics` endpoint is plain text exposition — Prometheus,
VictoriaMetrics, vmagent, and Grafana Alloy all scrape it unchanged.

Minimal `vmagent`/VictoriaMetrics scrape config (`-promscrape.config`):

```yaml
scrape_configs:
  - job_name: edda-api
    scrape_interval: 15s
    static_configs:
      - targets: ["localhost:8787"]
```

Point Grafana at VictoriaMetrics as a Prometheus data source
(`http://localhost:8428`).

## The alerts that matter

| Signal | Query sketch | Why |
|---|---|---|
| Ingest gone quiet | `time() - edda_eddn_last_apply_unix_seconds > 300` | The 23-silent-hours wedge: feed up, writer wedged, nothing landing. |
| Sink backpressure | `edda_eddn_queue_depth / edda_eddn_queue_capacity > 0.9` | Wedge #2: writer stuck (poison batch, DB down) while the feed keeps producing. |
| Data loss | `increase(edda_eddn_batches_total{outcome="dropped"}[1h]) > 0` | Poison batches being dropped; boards self-heal on the next observation, but a stream of these is a schema change. |
| Feed flapping | `increase(edda_eddn_reconnects_total[15m]) > 5` | Relay trouble or the idle-wedge firing repeatedly. |
| DB saturation | `edda_db_pool_idle == 0` sustained | Pool exhausted; queries queueing. |
| Serving errors | `increase(edda_http_requests_total{status=~"5.."}[5m]) > 0` | Any 5xx on manifest/artifact/stars is a client-visible failure. |

## Client (the desktop app)

The app wires the same facade and collects the same-shaped series
locally (plot walls, journal events) but **emits nothing off the box**
— telemetry stays on the user's machine unless we ever ship an
explicit opt-in. The routing series names (`edda_route_requests_total`,
`edda_route_wall_seconds`) are shared with the future server-side
routing endpoint so dashboards carry over.
