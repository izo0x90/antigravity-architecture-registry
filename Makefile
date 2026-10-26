.PHONY: start stop status clean restart log lint check ui sandbox-dev sandbox-clean dev-setup

# The tools `make dev-setup` installs (in ~/.local/bin, unless on PATH).
TOOLS ?= $(HOME)/.local/bin
AST_GREP ?= $(shell which ast-grep 2>/dev/null || echo "$(TOOLS)/ast-grep")
GITLEAKS ?= $(shell which gitleaks 2>/dev/null || echo "$(TOOLS)/gitleaks")
CARGO_DENY ?= $(shell which cargo-deny 2>/dev/null || echo "$(TOOLS)/cargo-deny")
# Relative to meta-harness/ (cargo's default); overridden by sandbox-dev.
CARGO_TARGET_DIR ?= target
export CARGO_TARGET_DIR
PID_FILE = mcp_server.pid
LOG_FILE = mcp_server.log

# The development tools at CI's versions, and the git hooks (a secret
# check on each commit, `make lint` on each push). Once per machine.
dev-setup:
	meta-harness/scripts/dev-setup.sh

lint:
	@echo "==> rustfmt: every Rust file formatted (rustfmt.toml)"
	cd meta-harness && cargo fmt --all --check
	@echo "==> Clippy: the server and every crate"
	cd meta-harness && cargo clippy --workspace --all-targets -- -D warnings
	@echo "==> Clippy: the cockpit's browser build"
	cd meta-harness && cargo clippy -p cockpit-ui --target wasm32-unknown-unknown -- -D warnings
	@echo "==> ast-grep: the structural rules"
	$(AST_GREP) scan
	@echo "==> gitleaks: no secrets in anything committed"
	$(GITLEAKS) git --redact --no-banner
	@echo "==> cargo-deny: the dependencies (security, licences, sources)"
	cd meta-harness && $(CARGO_DENY) check

check: lint
	@echo "==> Running Cargo Test in meta-harness..."
	cd meta-harness && cargo test --workspace

ui:
	@echo "==> Building Cockpit UI WASM..."
	cd meta-harness && cargo build -p cockpit-ui --target wasm32-unknown-unknown --release
	cd meta-harness && wasm-bindgen $(CARGO_TARGET_DIR)/wasm32-unknown-unknown/release/cockpit_ui.wasm --out-dir static/wasm --target web

# --- Sandbox only -------------------------------------------------------------
# For the agent sandbox (Linux), not your Mac. Builds into a sandbox-local target
# dir (meta-harness/target holds host artifacts), starts sbx-app-bridge, and runs the
# server on 127.0.0.1:8080. Open http://sandbox.localhost:8080 in the host Chrome.
SANDBOX_TARGET_DIR ?= $(HOME)/.cache/meta-harness-sandbox-target
# The sandbox disk is small: skip debug info and incremental caches (~1/3 the size).
sandbox-dev: export CARGO_PROFILE_DEV_DEBUG = 0
sandbox-dev: export CARGO_INCREMENTAL = 0

sandbox-dev:
	$(MAKE) ui CARGO_TARGET_DIR=$(SANDBOX_TARGET_DIR)
	sbx-app-bridge start
	cd meta-harness && CARGO_TARGET_DIR=$(SANDBOX_TARGET_DIR) \
		STATIC_DIR=static NAH_REPO_ROOT=. \
		HOST=127.0.0.1 PORT=8080 cargo run --bin meta-harness


start:
	@if [ -f $(PID_FILE) ] && kill -0 $$(cat $(PID_FILE)) 2>/dev/null; then \
		echo "MCP Server is already running (PID: $$(cat $(PID_FILE)))"; \
	else \
		echo "Starting MCP Server in background..."; \
		tail -f /dev/null | uv run skills/architecture-registry/mcp_server.py > $(LOG_FILE) 2>&1 & echo $$! > $(PID_FILE); \
		sleep 1; \
		if kill -0 $$(cat $(PID_FILE)) 2>/dev/null; then \
			echo "MCP Server started successfully (PID: $$(cat $(PID_FILE)))"; \
		else \
			echo "Failed to start MCP Server. Check $(LOG_FILE)"; \
			rm -f $(PID_FILE); \
		fi \
	fi

stop:
	@if [ -f $(PID_FILE) ]; then \
		PID=$$(cat $(PID_FILE)); \
		echo "Stopping MCP Server (PID: $$PID)..."; \
		kill $$PID || kill -9 $$PID 2>/dev/null; \
		rm -f $(PID_FILE); \
		echo "MCP Server stopped."; \
	else \
		echo "No running MCP Server found (missing $(PID_FILE))."; \
	fi

status:
	@if [ -f $(PID_FILE) ] && kill -0 $$(cat $(PID_FILE)) 2>/dev/null; then \
		echo "MCP Server is RUNNING (PID: $$(cat $(PID_FILE)))"; \
	else \
		echo "MCP Server is STOPPED"; \
	fi

restart: stop start

log:
	@if [ -f $(LOG_FILE) ]; then \
		tail -n 50 $(LOG_FILE); \
	else \
		echo "No log file found."; \
	fi

# Frees the sandbox's disk: only what's throwaway (download caches, build
# leftovers, old temp and scratch files). Safe to run any time.
# DEEP=1 also wipes the whole build cache (the next build takes ~10 min).
sandbox-clean:
	SANDBOX_TARGET_DIR=$(SANDBOX_TARGET_DIR) bash meta-harness/scripts/sandbox-clean.sh $(if $(filter 1,$(DEEP)),--deep,)

clean: stop
	rm -f $(LOG_FILE)
