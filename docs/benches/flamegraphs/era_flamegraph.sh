#!/usr/bin/env bash
# Item 26: era flamegraphs. Checks out each era commit into a throwaway
# worktree, builds its bench with release debuginfo, and records an ETW
# flamegraph (needs an ELEVATED shell; cargo-flamegraph rides blondie on
# Windows) for each signature cell. SVGs + a times.csv land in
# docs/benches/flamegraphs/runs/<timestamp>/.
#
# Caveat, by design: every era runs against TODAY'S index and sidecars,
# so the profiles isolate code shape across eras -- they do not
# reproduce archived-era routes bit-for-bit (sidecar contents evolved).
#
#   docs/benches/flamegraphs/era_flamegraph.sh [label]
set -u
ROOT="${EDDA_ROOT:-$(git rev-parse --show-toplevel)}"
cd "$ROOT" || exit 1
TS=$(date +%Y%m%d-%H%M%S)
RUN="$ROOT/docs/benches/flamegraphs/runs/${TS}${1:+-$1}"
mkdir -p "$RUN"

# label|commit  (HEAD uses the main tree's checkout)
ERAS=(
  "e1-pre-goal-field|181591e"
  "e2-seconds-judging|ea2b641"
  "e3-offramp-wavecost|69d8b66"
  "e4-head|HEAD"
)
# label|extra bench args (cell + ship + budget)
CELLS=(
  "sol-e|Sol|Sagittarius A*|--thorough --budget 60"
  "cs-back-e|Spase AA-A a108-0|Colonia|--thorough --budget 60"
  "beagle-m|Wongi|Beagle Point|--ship mandalay --thorough --budget 60"
)

echo "era,cell,jumps,stops,wall_ms,svg" > "$RUN/times.csv"
for era in "${ERAS[@]}"; do
  IFS='|' read -r ELABEL COMMIT <<< "$era"
  if [ "$COMMIT" = "HEAD" ]; then
    SRC="$ROOT"
  else
    SRC="$ROOT/.claude/worktrees/fg-$ELABEL"
    git worktree add -d "$SRC" "$COMMIT" >/dev/null 2>&1 || { echo "worktree $COMMIT failed"; continue; }
  fi
  TGT="$ROOT/.claude/worktrees/target-fg-$ELABEL"
  echo "== $ELABEL ($COMMIT): building with debuginfo =="
  (cd "$SRC" && CARGO_PROFILE_RELEASE_DEBUG=true CARGO_TARGET_DIR="$TGT" \
    cargo build --release -p ed-galaxy --example bench 2>&1 | tail -1)
  EXE="$TGT/release/examples/bench.exe"
  [ -f "$EXE" ] || { echo "  no exe, skipping era"; continue; }
  for cell in "${CELLS[@]}"; do
    IFS='|' read -r CLABEL FROM TO EXTRA <<< "$cell"
    SVG="$RUN/$ELABEL--$CLABEL.svg"
    echo "  -- $CLABEL"
    # flamegraph runs the exe under ETW sampling. Capture FULL output --
    # a head-truncated pipe kills flamegraph before it writes the SVG
    # (the first run of this script produced 12 rows and zero graphs).
    FULL=$(cd "$ROOT" && flamegraph -o "$SVG" -- "$EXE" .data/galaxy "$FROM" "$TO" $EXTRA 2>&1)
    OUT=$(echo "$FULL" | grep -E "^[0-9]+ jumps" | head -1)
    echo "     $OUT"
    [ -f "$SVG" ] || echo "     WARNING: no SVG written; flamegraph said: $(echo "$FULL" | tail -2)"
    J=$(echo "$OUT" | grep -oE '^[0-9]+' | head -1)
    S=$(echo "$OUT" | grep -oE '[0-9]+ refuel' | grep -oE '[0-9]+')
    W=$(echo "$OUT" | grep -oE 'wall [0-9]+' | grep -oE '[0-9]+')
    echo "$ELABEL,$CLABEL,${J:-},${S:-},${W:-},$(basename "$SVG")" >> "$RUN/times.csv"
  done
  if [ "$COMMIT" != "HEAD" ]; then
    git worktree remove --force "$SRC" >/dev/null 2>&1
  fi
done
echo "run complete: $RUN"
