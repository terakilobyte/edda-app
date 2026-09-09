#!/usr/bin/env python3
"""The uber-experiment: optuna search over the planner's knobs.

Every trial spawns bench.exe per signature cell with the knob ARGS
(args first, env fallback -- the configuration is visible in every
command line) and scores the trial in pilot seconds:

    score(cell) = wall_s + jumps * 40 + refuel_stops * 120

Wall seconds and flying seconds are the same currency -- the CMDR
spends both once. A failed cell (no route / crash / timeout) costs its
budget plus a 20,000 s penalty so the optimizer never mistakes a crash
for a shortcut.

The search space is seeded with the shipped defaults (trial 0) and
bracketed by what the campaign already measured: C_cap floors at 4
(below measurably costs refuel stops), K_field spans 2..16 because the
radial density curve puts rim cells at ~2 and arm cells at 10+,
B_ratio brackets the 3 that already transformed the crossings.

Usage:
    python docs/benches/knobs/knobsearch.py --trials 40
    python docs/benches/knobs/knobsearch.py --report   # top trials so far

Results: sqlite study + trials.jsonl beside this script. Whatever the
optimizer crowns must still pass the full sweep matrix before any
default changes -- the ML proposes, the benchmark disposes.
"""

import os
import argparse
import os
import json
import pathlib
import subprocess
import sys
import time

HERE = pathlib.Path(__file__).resolve().parent
REPO = HERE.parents[2]
DEFAULT_BENCH = os.environ.get("EDDA_BENCH", os.path.join("target", "release", "examples", "bench" + (".exe" if os.name == "nt" else "")))
INDEX = ".data/galaxy"

# The signature cells: dense mid-run, core slog, rim crossing, desert
# both ways, and THREE Mandalay cells (per-class fitting on two data
# points is not fitting). All --thorough (app parity). The last field
# is REPEATS: volatile cells (deep-dig variance -- the lesson of v2's
# non-decomposing champion and retro's autopsy) are run N times and
# scored by their median.
CELLS = [
    ("Wongi", "Colonia", "explorer", 15, 1),
    ("Wongi", "Colonia", "mandalay", 15, 1),
    ("Sol", "Sagittarius A*", "mandalay", 15, 1),
    ("Wongi", "Beagle Point", "explorer", 30, 3),
    ("Colonia", "Spase AA-A a108-0", "explorer", 30, 1),
    ("Spase AA-A a108-0", "Colonia", "explorer", 30, 3),
    ("Colonia", "Spase AA-A a108-0", "mandalay", 30, 1),
]

# Objective v4 (item 22): coefficients JOURNAL-FIT from the commander's
# own flying (docs/benches/knobs/journal_fit.py, 2026-09-02: 436 plain
# gaps, 745 scooping gaps across 65 journals). A jump is 74 s in the
# Caspian and 64 s in the Mandalay (median cadence); a refuel costs
# ~32 s approach overhead plus tonnes over the scoop's hardware rate.
# v1-v3 used flat 40 s/jump + 120 s/stop -- scores are NOT comparable
# across the objective change.
JUMP_S = {"explorer": 74.0, "explorer86": 74.0, "mandalay": 64.0}
STOP_OVERHEAD_S = 36.0
SCOOP_RATE = {"explorer": 1.245, "explorer86": 1.245, "mandalay": 0.577}
FAIL_S = 20_000.0


def run_cell(bench, cell, knob_args):
    frm, to, ship, budget = cell[:4]
    cmd = [bench, INDEX, frm, to, "--ship", ship, "--thorough", "--budget", str(budget), "--json", *knob_args]
    try:
        out = subprocess.run(cmd, cwd=REPO, capture_output=True, text=True, timeout=budget + 60)
    except subprocess.TimeoutExpired:
        return None, {"error": "timeout", "cmd": " ".join(cmd)}
    for line in reversed(out.stdout.splitlines()):
        line = line.strip()
        if line.startswith("{"):
            try:
                return json.loads(line), None
            except json.JSONDecodeError:
                break
    return None, {"error": "no-json", "rc": out.returncode, "cmd": " ".join(cmd)}


def score_cell(r, budget, ship):
    if r is None:
        return budget + FAIL_S
    t_jump = JUMP_S.get(ship, 70.0)
    if "refuel_tonnes" in r:
        refuel = r["refuel_stops"] * STOP_OVERHEAD_S + r["refuel_tonnes"] / SCOOP_RATE.get(ship, 1.0)
    else:
        refuel = r["refuel_stops"] * 120.0
    return r["wall_ms"] / 1000.0 + r["jumps"] * t_jump + refuel


def knob_args_for(params, ship):
    args = []
    # Per-boost-class cone start angle: x4 ships get their own fit.
    t1 = params["C_theta1_x4"] if ship == "mandalay" else params["C_theta1_x6"]
    g = params["C_growth"]
    angles = f"{t1:.1f},{t1 * g:.1f},{t1 * g * g:.1f}"
    args += ["--cone-angles", angles, "--cone-cap", str(params["C_cap"]), "--cone-band", f"{params['C_band']:.2f}"]
    args += ["--greedy-floor", str(params["K_floor"])]
    args += ["--field-min", str(params["K_field"])]
    args += ["--bidi-ratio", str(params["B_ratio"]), "--bidi-offset", str(params["B_offset"])]
    args += ["--meet-settle", str(params["B_meet_settle"])]
    args += ["--floor-slack", f"{params['F_slack']:.2f}", "--field-floor-slack", f"{params['F_field_slack']:.2f}"]
    args += ["--floor-pad", f"{params['F_pad']:.1f}", "--floor-trust-ratio", f"{params['F_trust']:.2f}"]
    args += ["--prize-k", f"{params['P_k']:.2f}"]
    return args


def objective_factory(bench, log_path):
    def objective(trial):
        # C_on FROZEN ON (off was matrix-rejected in v1; unanimous on in
        # v2). Frozen flats: C_cap=8, K_floor=512 (reach-cubed normalized
        # in-engine), B_*=3/64/800, F_pad=2.
        params = {"C_on": True, "C_cap": 8, "K_floor": 512}
        params["C_theta1_x6"] = trial.suggest_float("C_theta1_x6", 5.0, 25.0)
        params["C_theta1_x4"] = trial.suggest_float("C_theta1_x4", 5.0, 25.0)
        params["C_growth"] = trial.suggest_float("C_growth", 1.0, 2.0)
        params["C_band"] = trial.suggest_float("C_band", 0.5, 0.95)
        params["K_field"] = trial.suggest_int("K_field", 2, 16)
        params["B_ratio"] = 3
        params["B_offset"] = 64
        params["B_meet_settle"] = 800
        # 19a: the credibility ceiling family (euclid slack, tight field
        # slack, pad, and how far off-reference a ship may be before the
        # field bound is distrusted).
        params["F_slack"] = trial.suggest_float("F_slack", 1.2, 1.6)
        params["F_field_slack"] = trial.suggest_float("F_field_slack", 1.02, 1.30)
        params["F_pad"] = 2.0
        params["F_trust"] = trial.suggest_float("F_trust", 1.0, 2.0)
        params["P_k"] = trial.suggest_float("P_k", 0.25, 4.0, log=True)

        total, cells = 0.0, []
        pruned = False
        for i, cell in enumerate(CELLS):
            knob_args = knob_args_for(params, cell[2])
            repeats = cell[4]
            runs = []
            for _ in range(repeats):
                r, err = run_cell(bench, cell, knob_args)
                runs.append((score_cell(r, cell[3], cell[2]), r, err))
            runs.sort(key=lambda x: x[0])
            s, r, err = runs[len(runs) // 2]
            total += s
            cells.append({"cell": f"{cell[0]}->{cell[1]}[{cell[2]}]", "score_s": round(s, 1),
                          "repeats": repeats, "spread_s": round(runs[-1][0] - runs[0][0], 1),
                          "jumps": r and r["jumps"], "stops": r and r["refuel_stops"],
                          "wall_ms": r and r["wall_ms"], "err": err})
            # A trial already worse at cell i than the median finisher was
            # at cell i has no business finishing its expensive tail.
            trial.report(total, step=i)
            if trial.should_prune() and i < len(CELLS) - 1:
                pruned = True
                break
        with open(log_path, "a", encoding="utf-8") as f:
            f.write(json.dumps({"trial": trial.number, "study": "v4", "params": params, "total_s": round(total, 1),
                                "pruned": pruned, "cells": cells,
                                "at": time.strftime("%Y-%m-%dT%H:%M:%S")}) + "\n")
        if pruned:
            import optuna as _o
            raise _o.TrialPruned()
        return total

    return objective


def write_run_report(study, note=""):
    """Every run leaves a timestamped folder: interactive HTML plots,
    a markdown summary, and the raw trials as CSV -- chronologically
    sortable by folder name."""
    import optuna.visualization as vis

    done = [t for t in study.trials if t.value is not None]
    if not done:
        print("no completed trials; no report written")
        return None
    stamp = time.strftime("%Y-%m-%dT%H%M%S")
    run_dir = HERE / "runs" / stamp
    run_dir.mkdir(parents=True, exist_ok=True)

    figs = [("optimization-history", vis.plot_optimization_history(study))]
    try:
        figs.append(("param-importances", vis.plot_param_importances(study)))
    except Exception as e:
        print(f"  (importances skipped: {e})")
    try:
        figs.append(("slices", vis.plot_slice(study)))
    except Exception as e:
        print(f"  (slices skipped: {e})")
    with open(run_dir / "report.html", "w", encoding="utf-8") as f:
        f.write(f"<h1>Planner knob search — {stamp}</h1><p>{note}</p>\n")
        for i, (name, fig) in enumerate(figs):
            f.write(f"<h2>{name}</h2>\n")
            f.write(fig.to_html(full_html=False, include_plotlyjs=(i == 0)))

    ranked = sorted(done, key=lambda t: t.value)
    base = next((t for t in done if t.number == 0), None)
    with open(run_dir / "summary.md", "w", encoding="utf-8") as f:
        f.write(f"# Knob search run {stamp}\n\n{note}\n\n")
        f.write(f"{len(done)} completed trials. Objective: pilot seconds over {len(CELLS)} signature cells "
                f"(wall + jumps x journal cadence + {STOP_OVERHEAD_S:.0f} s/stop + tonnes/scoop-rate).\n\n")
        if base:
            f.write(f"Baseline (shipped defaults, trial 0): **{base.value:.0f} s**\n\n")
        f.write("| rank | trial | pilot-s | Δ vs baseline | params |\n|---|---|---|---|---|\n")
        for rank, t in enumerate(ranked[:10], 1):
            delta = f"{t.value - base.value:+.0f}" if base else "-"
            f.write(f"| {rank} | #{t.number} | {t.value:.0f} | {delta} | `{t.params}` |\n")
    with open(run_dir / "trials.csv", "w", encoding="utf-8") as f:
        keys = sorted({k for t in done for k in t.params})
        f.write("trial,pilot_s," + ",".join(keys) + "\n")
        for t in sorted(done, key=lambda t: t.number):
            f.write(f"{t.number},{t.value:.1f}," + ",".join(str(t.params.get(k, "")) for k in keys) + "\n")
    print(f"report: {run_dir}")
    return run_dir


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--trials", type=int, default=40)
    ap.add_argument("--bench", default=DEFAULT_BENCH)
    ap.add_argument("--report", action="store_true")
    ap.add_argument("--note", default="")
    args = ap.parse_args()

    import optuna

    optuna.logging.set_verbosity(optuna.logging.WARNING)
    study = optuna.create_study(
        study_name="planner-knobs-v4",
        storage=f"sqlite:///{(HERE / 'knobsearch.db').as_posix()}",
        load_if_exists=True,
        direction="minimize",
        pruner=optuna.pruners.MedianPruner(n_startup_trials=8, n_warmup_steps=1),
    )
    if args.report:
        write_run_report(study, args.note or "report-only snapshot")
        return

    if not any(t.number == 0 for t in study.trials):
        # Trial 0 = the shipped defaults, so every gain is measured
        # against what we actually run today.
        study.enqueue_trial({
            "C_theta1_x6": 10.0, "C_theta1_x4": 10.0, "C_growth": 1.5, "C_band": 0.8,
            "K_field": 2, "F_slack": 1.25, "F_field_slack": 1.04, "F_trust": 1.3, "P_k": 1.0,
        })
    study.optimize(objective_factory(args.bench, HERE / "trials.jsonl"), n_trials=args.trials)
    print(f"best: {study.best_value:.0f} pilot-s with {study.best_params}")
    write_run_report(study, args.note or f"{args.trials}-trial run")


if __name__ == "__main__":
    main()
