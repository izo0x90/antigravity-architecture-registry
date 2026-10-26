#!/bin/bash
# Installs the development tools `make lint` and the git hooks use, at the
# versions CI uses, and turns the repository's git hooks on. Run once per
# machine (`make dev-setup`); running it again only fixes what's missing.
#
#   gitleaks    no secrets in what's committed
#   cargo-deny  dependencies: known security holes, licences
#   ast-grep    the structural rules in rules/ast-grep
#   rustup      clippy, rustfmt, rust-analyzer, the browser build's target
#
# Downloads are checked against the checksums pinned here. Tools go in
# ~/.local/bin (NAH_TOOLS to change it).
set -euo pipefail
cd "$(dirname "$0")/../.."

TOOLS="${NAH_TOOLS:-$HOME/.local/bin}"
mkdir -p "$TOOLS"

GITLEAKS_VERSION=8.30.1
CARGO_DENY_VERSION=0.20.2
AST_GREP_VERSION=0.45.3

case "$(uname -s)-$(uname -m)" in
  Linux-aarch64)  gl=linux_arm64;  gl_sum=e4a487ee7ccd7d3a7f7ec08657610aa3606637dab924210b3aee62570fb4b080
                  cd_t=aarch64-unknown-linux-musl; cd_sum=995c82be0defc7a025cae49a2aa2644ce8245c9a3318fc4103907c6a285e8c7d ;;
  Linux-x86_64)   gl=linux_x64;    gl_sum=551f6fc83ea457d62a0d98237cbad105af8d557003051f41f3e7ca7b3f2470eb
                  cd_t=x86_64-unknown-linux-musl;  cd_sum=9f12ed4c49936e09b48bf862b595cde2fe64fcbd9d74dfacac6131ca824c8d5f ;;
  Darwin-arm64)   gl=darwin_arm64; gl_sum=b40ab0ae55c505963e365f271a8d3846efbc170aa17f2607f13df610a9aeb6a5
                  cd_t=aarch64-apple-darwin;       cd_sum=fe67d82a10d8597a3549364cb733a3f9cc1bfff9031b7ae46384a9f2a72090c3 ;;
  Darwin-x86_64)  gl=darwin_x64;   gl_sum=dfe101a4db2255fc85120ac7f3d25e4342c3c20cf749f2c20a18081af1952709
                  cd_t=x86_64-apple-darwin;        cd_sum=248da7f581724e470071990c088ffc55c811981715f4cbdb258621fb79f8b7a6 ;;
  *) echo "dev-setup: no pinned tools for $(uname -s) $(uname -m)" >&2; exit 1 ;;
esac

sha256() { if command -v sha256sum >/dev/null; then sha256sum "$1"; else shasum -a 256 "$1"; fi | awk '{print $1}'; }

# Downloads `url`, checks it against `sum`, and puts the program `name`
# from the archive in the tools folder.
fetch() {
  local name="$1" url="$2" sum="$3" tmp
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' RETURN
  curl -fsSL "$url" -o "$tmp/archive.tar.gz"
  if [ "$(sha256 "$tmp/archive.tar.gz")" != "$sum" ]; then
    echo "dev-setup: $name's download doesn't match its pinned checksum" >&2
    return 1
  fi
  tar -xzf "$tmp/archive.tar.gz" -C "$tmp"
  install -m 0755 "$(find "$tmp" -type f -name "$name" | head -1)" "$TOOLS/$name"
}

have() { [ -x "$TOOLS/$1" ] && "$TOOLS/$1" "${@:2}" 2>/dev/null | grep -q "$3"; }

if have gitleaks version "$GITLEAKS_VERSION"; then
  echo "gitleaks $GITLEAKS_VERSION: installed"
else
  fetch gitleaks "https://github.com/gitleaks/gitleaks/releases/download/v$GITLEAKS_VERSION/gitleaks_${GITLEAKS_VERSION}_$gl.tar.gz" "$gl_sum"
  echo "gitleaks $GITLEAKS_VERSION: installed now"
fi

if have cargo-deny --version "$CARGO_DENY_VERSION"; then
  echo "cargo-deny $CARGO_DENY_VERSION: installed"
else
  fetch cargo-deny "https://github.com/EmbarkStudios/cargo-deny/releases/download/$CARGO_DENY_VERSION/cargo-deny-$CARGO_DENY_VERSION-$cd_t.tar.gz" "$cd_sum"
  echo "cargo-deny $CARGO_DENY_VERSION: installed now"
fi

if have ast-grep --version "$AST_GREP_VERSION"; then
  echo "ast-grep $AST_GREP_VERSION: installed"
else
  # (Built from source: a few minutes, once.)
  cargo install ast-grep --version "$AST_GREP_VERSION" --locked --root "$(dirname "$TOOLS")"
  echo "ast-grep $AST_GREP_VERSION: installed now"
fi

if command -v rustup >/dev/null; then
  rustup component add clippy rustfmt rust-analyzer >/dev/null
  rustup target add wasm32-unknown-unknown >/dev/null
  echo "rustup: clippy, rustfmt, rust-analyzer, wasm32 target"
fi

# The repository's hooks: secrets and formatting on each commit, the checks
# that build nothing (`make lint-light`) on each push.
git config core.hooksPath .githooks
echo "git hooks: on (.githooks)"

case ":$PATH:" in
  *":$TOOLS:"*) ;;
  *) echo "Add $TOOLS to your PATH (the Makefile finds the tools there either way)." ;;
esac
