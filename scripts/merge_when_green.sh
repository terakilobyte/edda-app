#!/bin/bash
# scripts/merge_when_green.sh <pr-number> [required-check-count]
# Merge a PR only after its checks EXIST and ALL pass. Two earlier gates in
# this session passed vacuously: one looked before GitHub had registered
# any checks, the other parsed `gh pr checks` text and miscounted "pending".
# This one reads JSON, waits for the expected number of checks to be
# present, then for none to be pending, then refuses on any failure.
set -euo pipefail
n="$1"; want="${2:-3}"
for i in $(seq 1 30); do
  have=$(gh pr view "$n" --json statusCheckRollup -q '.statusCheckRollup | length' 2>/dev/null || echo 0)
  [ "${have:-0}" -ge "$want" ] && break
  sleep 20
done
[ "${have:-0}" -ge "$want" ] || { echo "only $have checks registered after 10 min (wanted $want) — not merging"; exit 1; }
for i in $(seq 1 120); do
  pending=$(gh pr view "$n" --json statusCheckRollup -q '[.statusCheckRollup[] | select((.conclusion // "") == "" or .status == "IN_PROGRESS" or .status == "QUEUED" or .status == "PENDING")] | length')
  [ "$pending" -eq 0 ] && break
  sleep 30
done
gh pr view "$n" --json statusCheckRollup -q '.statusCheckRollup[] | "  \(.name // .context): \(.conclusion // .status)"'
bad=$(gh pr view "$n" --json statusCheckRollup -q '[.statusCheckRollup[] | select((.conclusion // "") as $c | $c != "SUCCESS" and $c != "NEUTRAL" and $c != "SKIPPED")] | length')
[ "$bad" -eq 0 ] || { echo "$bad check(s) not green — not merging"; exit 1; }
gh pr merge "$n" --squash --admin --delete-branch >/dev/null 2>&1 || true
for i in $(seq 1 36); do
  st=$(gh pr view "$n" --json state -q .state 2>/dev/null)
  [ "$st" = "MERGED" ] && break
  [ $((i % 6)) -eq 0 ] && gh pr merge "$n" --squash --admin --delete-branch >/dev/null 2>&1 || true
  sleep 10
done
echo "PR $n: $st"
[ "$st" = "MERGED" ]
