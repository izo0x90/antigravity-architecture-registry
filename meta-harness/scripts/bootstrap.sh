#!/usr/bin/env bash
# Builds the meta-harness binary, then installs the packs named, e.g.
#   scripts/bootstrap.sh antigravity-acp rust-analyzer
# Everything about a pack (where it comes from, its checksums, how it's
# checked) is in its pack.toml; `meta-harness pack list` shows them all.
set -euo pipefail
cd "$(dirname "$0")/.."
if [[ $# -eq 0 ]]; then
  echo "usage: $0 <pack>... (see: meta-harness pack list)" >&2
  exit 2
fi
cargo build --release --bin meta-harness
for pack in "$@"; do
  target/release/meta-harness pack install "$pack"
done
