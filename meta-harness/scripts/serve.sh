#!/usr/bin/env bash
# Starts the meta-harness server in the background over a project (default:
# this repository), from the release build (built if it's missing), and
# prints the cockpit's link. Its output goes to ~/.cache/nah/serve.log; run
# again to restart it.
#   scripts/serve.sh [project]
set -euo pipefail
cd "$(dirname "$0")/.."
project="$(cd "${1:-..}" && pwd)"
state="${XDG_CACHE_HOME:-$HOME/.cache}/nah"
mkdir -p "$state"
if [[ ! -x target/release/meta-harness ]]; then
  cargo build --release --bin meta-harness
fi
if [[ ! -f static/wasm/cockpit_ui_bg.wasm ]]; then
  echo "the cockpit isn't built: run make ui first" >&2
  exit 1
fi
if [[ -f "$state/serve.pid" ]] && kill -0 "$(cat "$state/serve.pid")" 2>/dev/null; then
  kill "$(cat "$state/serve.pid")"
  sleep 1
fi
NAH_REPO_ROOT="$project" STATIC_DIR="${STATIC_DIR:-static}" HOST="${HOST:-127.0.0.1}" PORT="${METAHARNESS_PORT:-${PORT:-8080}}" \
  setsid nohup target/release/meta-harness >"$state/serve.log" 2>&1 </dev/null &
echo $! >"$state/serve.pid"
for _ in $(seq 1 60); do
  if link=$(grep -o 'http://[^ ]*/cockpit?nah=[^ ]*' "$state/serve.log" 2>/dev/null); then
    echo "Open the cockpit: $link"
    exit 0
  fi
  sleep 1
done
echo "the server didn't print its link; see $state/serve.log" >&2
exit 1
