#!/usr/bin/env python3
"""Dashboard no-data audit (2026-09-20).

For every panel target in deploy/monitoring/dashboards/*.json, answer
"why would this read No data?" from the repo alone, and — when a Grafana
credential is supplied — from the live instance too.

Static columns (always):
  series      the metric names the expression references
  emitter     where the Rust tree emits each (counter!/gauge!/histogram!,
              or the telemetry / publication tables that build names)
  scrape_job  the vm-scrape.yml job that would carry it
  static      OK | absent:no-emitter | absent:no-scrape-job |
              label:never-emitted | sql (Postgres datasource)

Live columns (only with GRAFANA_USER/GRAFANA_PASSWORD, against
GRAFANA_URL, default http://localhost:3000 — the ssh tunnel):
  live_24h, live_7d   OK | no-data | error:<msg>

Usage:
  python3 docs/benches/knobs/dashboard_audit.py > docs/benches/<date>-dashboard-no-data-audit.csv
  GRAFANA_USER=admin GRAFANA_PASSWORD=... python3 docs/benches/knobs/dashboard_audit.py

The Postgres (feedback) panels are checked for column existence against
crates/ed-api/migrations/0008_feedback.sql; PromQL is never sent to a
SQL datasource.
"""
import base64
import csv
import glob
import json
import os
import re
import subprocess
import sys
import time
import urllib.request

ROOT = os.environ.get("EDDA_ROOT") or os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "..", ".."))
DASH = os.path.join(ROOT, "deploy", "monitoring", "dashboards")
SCRAPE = os.path.join(ROOT, "deploy", "monitoring", "vm-scrape.yml")

METRIC_RE = re.compile(r"\b((?:edda|node|pg)_[A-Za-z0-9_]+)\b")
MATCHER_RE = re.compile(r'(\w+)(=|=~|!=|!~)"([^"]*)"')
SUFFIXES = ("_bucket", "_count", "_sum")

# Names the code builds rather than writes literally.
DYNAMIC_EMITTERS = {
    "edda_client_search_age_hours": "crates/ed-api/src/telemetry.rs (search_metric)",
    "edda_client_router_gate_ly": "crates/ed-api/src/telemetry.rs (search_metric)",
    "edda_client_carrier_stats_age_hours": "crates/ed-api/src/telemetry.rs (search_metric)",
}


def sh(cmd):
    return subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True).stdout


def emitters():
    """metric name -> first file:line that emits it, from the Rust tree."""
    # Every quoted "edda_*" literal in the Rust tree: the macro name and
    # the series name are often on different lines (the telemetry
    # counters wrap), and metrics.rs publishes edda_publish_* from a
    # tuple table, so the literal is the honest unit. Doc comments use
    # backticks, never quotes, so they do not count.
    out = {}
    text = sh(["grep", "-rnoE", r'"edda_[a-z0-9_]+"', "crates", "src-tauri", "--include=*.rs"])
    for line in text.splitlines():
        m = re.search(r'^(.*?):(\d+):"(edda_[a-z0-9_]+)"', line)
        if m and m.group(3) not in out:
            out[m.group(3)] = f"{m.group(1)}:{m.group(2)}"
    out.update(DYNAMIC_EMITTERS)
    return out


def scrape_jobs():
    """metric prefix -> job name, from vm-scrape.yml (targets are loopback ports)."""
    jobs = {}
    port_to_prefix = {"8787": "edda_", "8788": "edda_", "9100": "node_", "9187": "pg_"}
    cur = None
    for line in open(SCRAPE):
        m = re.match(r"\s*-\s*job_name:\s*(\S+)", line)
        if m:
            cur = m.group(1)
        m = re.search(r'127\.0\.0\.1:(\d+)', line)
        if m and cur:
            prefix = port_to_prefix.get(m.group(1))
            if prefix:
                jobs.setdefault(prefix, cur)
    return jobs


def label_value_emitted(value):
    """Is this literal label value present anywhere in the Rust tree?"""
    needle = value.split("::")[1] if "::" in value else value
    text = sh(["grep", "-rlF", f'"{needle}', "crates", "src-tauri", "--include=*.rs"])
    return bool(text.strip())


def feedback_columns():
    sql = open(os.path.join(ROOT, "crates/ed-api/migrations/0008_feedback.sql")).read()
    body = sql.split("feedback (", 1)[1].split(");", 1)[0]
    return {l.strip().split()[0] for l in body.splitlines() if l.strip() and not l.strip().startswith("--")}


def base(name):
    for s in SUFFIXES:
        if name.endswith(s):
            return name[: -len(s)]
    return name


def classify_promql(expr, emit, jobs):
    names = sorted(set(METRIC_RE.findall(expr)))
    bases = sorted({base(n) for n in names})
    where = []
    verdict = "OK"
    for b in bases:
        prefix = b.split("_", 1)[0] + "_"
        job = jobs.get(prefix)
        src = emit.get(b)
        if prefix == "edda_":
            if not src:
                verdict = "absent:no-emitter"
                where.append(f"{b}=NONE")
                continue
            where.append(f"{b}={src}")
        else:
            where.append(f"{b}=exporter({prefix.rstrip('_')})")
        if not job:
            verdict = "absent:no-scrape-job"
    # Hard-coded label values on edda_ series that no code emits.
    if verdict == "OK" and any(b.startswith("edda_") for b in bases):
        for label, op, value in MATCHER_RE.findall(expr):
            if label == "le" or op.startswith("!"):
                continue
            probe = value.rstrip(".*").rstrip("::")
            if op == "=~" and "|" in probe:
                continue
            if not label_value_emitted(probe):
                verdict = f"label:never-emitted({label}={value})"
                break
    job_names = sorted({jobs.get(b.split("_", 1)[0] + "_", "-") for b in bases})
    return ";".join(bases), ";".join(where), ";".join(job_names), verdict


def classify_sql(raw, cols):
    used = set(re.findall(r"\b([a-z_]+)\b", raw))
    missing = sorted(c for c in ("id", "received_at", "version", "os", "body", "log_tail") if c in used and c not in cols)
    return "feedback", "crates/ed-api/migrations/0008_feedback.sql", "edda-pg", ("sql" if not missing else f"sql:missing-columns({','.join(missing)})")


class Live:
    def __init__(self):
        self.url = os.environ.get("GRAFANA_URL", "http://localhost:3000").rstrip("/")
        user, pw = os.environ.get("GRAFANA_USER"), os.environ.get("GRAFANA_PASSWORD")
        self.auth = base64.b64encode(f"{user}:{pw}".encode()).decode() if user and pw else None

    def query(self, ds_uid, expr, hours):
        if not self.auth:
            return "not-run(no credential)"
        now = int(time.time() * 1000)
        body = {"from": str(now - hours * 3600 * 1000), "to": str(now),
                "queries": [{"refId": "A", "datasource": {"uid": ds_uid}, "expr": expr,
                             "intervalMs": 60000, "maxDataPoints": 500}]}
        req = urllib.request.Request(f"{self.url}/api/ds/query", data=json.dumps(body).encode(),
                                     headers={"Content-Type": "application/json", "Authorization": f"Basic {self.auth}"})
        try:
            with urllib.request.urlopen(req, timeout=60) as r:
                res = json.load(r)["results"]["A"]
        except Exception as e:  # noqa: BLE001
            return f"error:{str(e)[:60]}"
        if res.get("error"):
            return f"error:{res['error'][:60]}"
        frames = res.get("frames", [])
        # A frame with no series comes back with values: [] — not [[]].
        points = sum(len(v[0]) if v else 0 for v in (f.get("data", {}).get("values", []) for f in frames))
        return "OK" if points else "no-data"


def main():
    emit, jobs, cols, live = emitters(), scrape_jobs(), feedback_columns(), Live()
    w = csv.writer(sys.stdout, lineterminator="\n")
    w.writerow(["dashboard", "panel_id", "panel", "datasource", "expr", "series", "emitter", "scrape_job", "static", "live_24h", "live_7d"])
    for f in sorted(glob.glob(os.path.join(DASH, "*.json"))):
        d = json.load(open(f))
        panels = []
        for p in d.get("panels", []):
            panels.append(p)
            panels.extend(p.get("panels", []))
        for p in panels:
            ds = (p.get("datasource") or {}).get("uid", "")
            for t in p.get("targets", []):
                expr = (t.get("expr") or t.get("rawSql") or "").strip()
                if not expr:
                    continue
                if ds == "edda-pg":
                    series, where, job, verdict = classify_sql(expr, cols)
                    l24 = l7 = "not-run(sql)"
                else:
                    series, where, job, verdict = classify_promql(expr, emit, jobs)
                    l24, l7 = live.query(ds, expr, 24), live.query(ds, expr, 24 * 7)
                w.writerow([d["uid"], p.get("id"), p.get("title"), ds, expr, series, where, job, verdict, l24, l7])


if __name__ == "__main__":
    main()
