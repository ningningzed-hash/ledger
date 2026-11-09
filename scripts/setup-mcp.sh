#!/bin/sh
# Install isolated, pinned financial MCP servers. Python 3.12+ is required.
set -eu
RUNTIME="${LEDGER_MCP_HOME:-$HOME/.local/share/ledger/mcp}"
PYTHON="${PYTHON:-python3.12}"
"$PYTHON" -c 'import sys; assert sys.version_info >= (3,12), "Python 3.12+ required"'
mkdir -p "$RUNTIME"
"$PYTHON" -m venv "$RUNTIME/aktools"
"$RUNTIME/aktools/bin/python" -m pip install --disable-pip-version-check 'aktools==0.1.4'
"$PYTHON" -m venv "$RUNTIME/yfinance"
"$RUNTIME/yfinance/bin/python" -m pip install --disable-pip-version-check 'yfmcp==0.14.0'
printf 'MCP runtime installed: %s\n' "$RUNTIME"
