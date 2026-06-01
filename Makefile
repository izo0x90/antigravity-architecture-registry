.PHONY: start stop status clean restart log

PID_FILE = mcp_server.pid
LOG_FILE = mcp_server.log

start:
	@if [ -f $(PID_FILE) ] && kill -0 $$(cat $(PID_FILE)) 2>/dev/null; then \
		echo "MCP Server is already running (PID: $$(cat $(PID_FILE)))"; \
	else \
		echo "Starting MCP Server in background..."; \
		tail -f /dev/null | uv run mcp_server.py > $(LOG_FILE) 2>&1 & echo $$! > $(PID_FILE); \
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

clean: stop
	rm -f $(LOG_FILE)
