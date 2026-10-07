"""Size the early-answer budget from full bench output.

Reads the per-route stdout of `examples/bench` (one file per route, as the
2026-10-07 matrix run wrote them: a "== From -> To (budget N)" header, then
"  found J jumps at T ms" per variant, then the final summary line) and
prints a CSV: per route, the first-found jumps and time, the best jumps
known at 5 s / 10 s / 15 s, the final jumps and time, and the jump cost of
answering at each cut-off.

    python docs/benches/knobs/sweep_first_found.py <dir-with-route-txt-files>
"""
import csv, glob, os, re, sys

d = sys.argv[1]
rows = []
for path in sorted(glob.glob(os.path.join(d, "*.txt"))):
    text = open(path, encoding="utf-8", errors="replace").read()
    head = re.search(r"^== (.+?) -> (.+?) \(budget (\d+)\)", text, re.M)
    if not head:
        continue
    frm, to, budget = head.group(1), head.group(2), int(head.group(3))
    founds = [(int(j), int(t)) for j, t in re.findall(r"found (\d+) jumps at (\d+) ms", text)]
    final = re.search(r"^(\d+) jumps, .*?, (\d+) ms \(wall (\d+) ms\), variants (\S+)", text, re.M)
    if not final:
        rows.append([frm, to, budget, "", "", "", "", "", "", "", "", "no route", ""])
        continue
    fj, fms, variants = int(final.group(1)), int(final.group(2)), final.group(4)
    first = founds[0] if founds else (fj, fms)
    def best_by(ms):
        c = [j for j, t in founds if t <= ms]
        return min(c) if c else ""
    b5, b10, b15 = best_by(5000), best_by(10000), best_by(15000)
    rows.append([frm, to, budget, first[0], first[1], b5, b10, b15, fj, fms, variants,
                 "" if b5 == "" else b5 - fj, "" if b10 == "" else b10 - fj])

w = csv.writer(sys.stdout, lineterminator="\n")
w.writerow(["from", "to", "budget_s", "first_found_jumps", "first_found_ms", "best_at_5s", "best_at_10s", "best_at_15s", "final_jumps", "final_ms", "variants", "extra_jumps_if_served_at_5s", "extra_jumps_if_served_at_10s"])
w.writerows(rows)
