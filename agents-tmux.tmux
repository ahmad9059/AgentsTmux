#!/usr/bin/env bash

PLUGIN_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if [[ -x "$PLUGIN_DIR/bin/agents-tmux" ]]; then
    SIDEBAR_BINARY="$PLUGIN_DIR/bin/agents-tmux"
elif [[ -x "$PLUGIN_DIR/target/release/agents-tmux" ]]; then
    SIDEBAR_BINARY="$PLUGIN_DIR/target/release/agents-tmux"
elif command -v "agents-tmux" &>/dev/null; then
    SIDEBAR_BINARY="agents-tmux"
fi

if [[ -z "$SIDEBAR_BINARY" ]]; then
    tmux run-shell -b "bash '$PLUGIN_DIR/install-wizard.sh'"
    exit 0
fi

INSTALLED_VERSION="$("$SIDEBAR_BINARY" version 2>/dev/null)"
EXPECTED_VERSION="$(sed -n 's/^version *= *"\(.*\)"/\1/p' "$PLUGIN_DIR/Cargo.toml")"

if [[ -n "$EXPECTED_VERSION" && "$INSTALLED_VERSION" != "$EXPECTED_VERSION" ]]; then
    tmux run-shell -b "SIDEBAR_UPDATE=1 bash '$PLUGIN_DIR/install-wizard.sh'"
    exit 0
fi

tmux set -g @agent_sidebar_bin "$SIDEBAR_BINARY"

tmux source-file "$PLUGIN_DIR/agent-sidebar.conf"

# Plug and play: wire up every detected agent (Claude Code, Codex, OpenCode V2,
# Pi). Idempotent, backs up changed files, and never blocks tmux startup.
# Disable with: set -g @agentstmux_auto_setup off
if [[ "$(tmux show-option -gqv @agentstmux_auto_setup)" != "off" ]]; then
    tmux run-shell -b "AGENTSTMUX_PLUGIN_DIR='$PLUGIN_DIR' '$SIDEBAR_BINARY' integrate --auto --quiet >/dev/null 2>&1"
fi
