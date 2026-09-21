#!/usr/bin/env bash
# Launch a built EDDA binary the way a commander does, let setup run to the
# end, and fail if it did not get there. The released 0.3.5 died in setup
# on every machine while every debug build the maintainer flew was fine
# (a plugin registered only under cfg(debug_assertions)); this runs the
# binary that ships, so that class of defect never ships again.
#
#   scripts/smoke-launch.sh <path-to-edda-binary>
#
# Passes when the process exits 0 AND its log carries "smoke: setup
# complete" — the exit code alone is not proof (a second instance exits 0
# by handing off to the first). Fails on a non-zero exit, a timeout, or a
# log without the line; prints the log's tail either way, so a PANIC line
# (lib.rs installs the hook) is in the CI output.
set -euo pipefail
bin="${1:?path to the edda binary}"
[[ -x "$bin" ]] || { echo "::error::$bin is not executable"; exit 1; }

case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*|Windows_NT) logs="${LOCALAPPDATA:?}/edda/logs" ;;
  Darwin) logs="$HOME/Library/Application Support/edda/logs" ;;
  *) logs="${XDG_DATA_HOME:-$HOME/.local/share}/edda/logs" ;;
esac
mkdir -p "$logs"
before="$(ls -1 "$logs" 2>/dev/null | wc -l)"

export EDDA_SMOKE_EXIT="${EDDA_SMOKE_EXIT:-5}"
export EDDA_NO_EDDN=1
export EDDA_NO_UPDATE=1
set +e
if command -v xvfb-run >/dev/null 2>&1 && [[ -z "${DISPLAY:-}" ]]; then
  timeout 120 xvfb-run -a "$bin"
else
  timeout 120 "$bin"
fi
code=$?
set -e

log="$(ls -1t "$logs"/edda.log* 2>/dev/null | head -1 || true)"
echo "--- exit $code; log: ${log:-none} (files before: $before)"
[[ -n "$log" ]] && tail -n 40 "$log"
if [[ $code -ne 0 ]]; then
  echo "::error::edda exited $code during the smoke launch (124 = timed out)"
  exit 1
fi
if [[ -z "$log" ]] || ! grep -q '"smoke: setup complete"' "$log"; then
  echo "::error::edda exited 0 but never logged 'smoke: setup complete' — setup did not run to the end"
  exit 1
fi
echo "smoke launch: setup complete, exited 0"
