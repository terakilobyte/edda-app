#!/usr/bin/env bash
# Single-account A/B: 0.4.8 (Store::sync) against this checkout (Registry::sync_all)
# on one frozen copy of a journal folder, alternating runs.
#   bash docs/benches/knobs/single-account-ab/run.sh <journal_dir> <scratch_dir> [runs=3]
# Needs: a v0.4.8 worktree at .claude/worktrees/ab-048 (created if missing),
# CARGO_TARGET_DIR for this tree (B) and edda-target-048 beside it for A.
set -euo pipefail
J="$1"; S="$2"; RUNS="${3:-3}"
ROOT="$(git rev-parse --show-toplevel)"; KNOB="$ROOT/docs/benches/knobs/single-account-ab"
OLD="$(git -C "$ROOT" rev-parse --git-common-dir)/../.claude/worktrees/ab-048"
mkdir -p "$S"; rm -rf "$S/journal"; cp -rp "$J" "$S/journal"
[ -d "$OLD" ] || git -C "$ROOT" worktree add --detach "$OLD" v0.4.8 >/dev/null
cp "$KNOB/old_store.rs" "$OLD/crates/ed-store/examples/ab_single.rs"
cp "$KNOB/new_registry.rs" "$ROOT/crates/ed-store/examples/ab_single.rs"
TB="${CARGO_TARGET_DIR:?set CARGO_TARGET_DIR}"; TA="$(dirname "$TB")/edda-target-048"
(cd "$OLD" && CARGO_TARGET_DIR="$TA" cargo build -q --release -p ed-store --example ab_single)
(cd "$ROOT" && CARGO_TARGET_DIR="$TB" cargo build -q --release -p ed-store --example ab_single)
for i in $(seq 1 "$RUNS"); do
  for side in A B; do
    if [ $side = A ]; then BIN="$TA/release/examples/ab_single"; else BIN="$TB/release/examples/ab_single"; fi
    rm -rf "$S/db-$side-$i"
    echo "== $side run $i"; "$BIN" "$S/journal" "$S/db-$side-$i" "$S/counts-$side-$i.csv"
  done
done
rm -f "$OLD/crates/ed-store/examples/ab_single.rs" "$ROOT/crates/ed-store/examples/ab_single.rs"
