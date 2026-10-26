#!/bin/bash
# Frees the sandbox's disk (20 GB): deletes only what's throwaway by
# definition, and says what it freed. Safe to run any time, as often as
# you like: each step skips what's already gone and what's still in use.
#
#   make sandbox-clean           # the throwaway (no rebuild needed after)
#   make sandbox-clean DEEP=1    # also the whole build cache (next build ~10 min)
#   scripts/sandbox-clean.sh --build-only   # the cargo wrapper's own call,
#                                           # when space runs low
#
# Never touched: the harness packs, the Rust toolchain, downloaded library
# sources, the current conversation's transcript, the server's data.
set -u

BUILD="${SANDBOX_TARGET_DIR:-$HOME/.cache/meta-harness-sandbox-target}"
DEEP=0; BUILD_ONLY=0; QUIET=0
for arg in "$@"; do
  case "$arg" in
    --deep) DEEP=1 ;;
    --build-only) BUILD_ONLY=1; QUIET=1 ;;
    *) echo "sandbox-clean: unknown option $arg" >&2; exit 2 ;;
  esac
done

free_kb() { df -k --output=avail / | tail -1; }
say() { [ "$QUIET" = 1 ] || echo "$@"; }
# Runs one step and says how much it freed.
step() {
  local what="$1"; shift
  local before; before=$(free_kb)
  "$@" >/dev/null 2>&1
  local freed=$(( ($(free_kb) - before) / 1024 ))
  [ "$freed" -gt 0 ] && say "  $what: $freed MB"
  return 0
}

# Our crates (their build output goes stale as the code changes).
OURS="meta_harness arch_service arch_store code_analyzer nah_packs arch_graph arch_search cockpit_ui harness_acp harness_core harness_contracts workspace_fs nah_lsp nah_changes nah_terminal nah_notify nah_state nah_timeline nah_access nah_tags nah_things nah_discussions"

build_leftovers() {
  local deps="$BUILD/debug/deps"
  [ -d "$deps" ] || return 0
  # Test programs: relinked in seconds when next needed.
  find "$deps" -maxdepth 1 -type f -perm -u+x ! -name "*.so" ! -name "*.rlib" ! -name "*.rmeta" -delete
  # Our own crates' older builds (the newest of each is kept by age).
  for c in $OURS; do
    find "$deps" -maxdepth 1 -name "*${c}-*" -mmin +60 -delete
  done
}

before=$(free_kb)
say "Free before: $(( before / 1024 )) MB"

step "build leftovers (test programs, our stale crates)" build_leftovers

if [ "$BUILD_ONLY" = 0 ]; then
  # Download caches: nothing reads them again once a thing is installed.
  step "npm download cache" rm -rf "$HOME/.npm/_cacache"
  step "Python download cache" rm -rf "$HOME/.cache/uv"
  # A headless browser nothing uses (live checks drive the user's Chrome).
  step "Playwright's browser" rm -rf "$HOME/.cache/ms-playwright"
  # The server's code-analysis caches for projects that are gone (one with
  # no note of its project is from before it kept one).
  step "old code-analysis caches" find "$HOME/.local/share/nah/cache" -mindepth 2 -maxdepth 2 -type d -mtime +1 ! -exec test -e "{}/project" \; -exec rm -rf {} +
  # Anything in /tmp a day old, but what's live: a folder holding a socket,
  # and Claude Code's own (this session's scratch is in it).
  step "temp files a day old" find /tmp -mindepth 1 -maxdepth 1 -mtime +0 ! -name "claude-*" ! -type s -exec sh -c 'for e; do [ -d "$e" ] && [ -n "$(find "$e" -type s -print -quit 2>/dev/null)" ] && continue; rm -rf "$e"; done' sh {} +
  # Screenshots are looked at as they're taken: an hour old, they're done.
  step "screenshots" find /tmp -path "/tmp/claude-*" -type f \( -name "*.png" -o -name "*.jpg" \) -mmin +60 -delete
  # Claude Code's data: old scratch folders, finished helpers' transcripts.
  step "old agent scratch folders" find "$HOME/.claude/jobs" -mindepth 3 -maxdepth 3 -path "*/tmp/*" -mtime +0 -exec rm -rf {} +
  step "finished helper transcripts" find "$HOME/.claude/projects" -path "*/subagents/*" -type f -mtime +0 -delete
fi

if [ "$DEEP" = 1 ]; then
  step "the whole build cache (next build ~10 min)" rm -rf "$BUILD"
fi

after=$(free_kb)
say "Free after:  $(( after / 1024 )) MB (freed $(( (after - before) / 1024 )) MB)"
